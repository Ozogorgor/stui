// init.go — Bubbletea Init plumbing for the ui controller.
// Wires up the IPC listener, poster-refresh poller, and the
// periodic tick commands the controller dispatches.

package ui

import (
	"fmt"
	"time"

	tea "charm.land/bubbletea/v2"

	"github.com/stui/stui/internal/ipc"
	"github.com/stui/stui/internal/ui/components"
	"github.com/stui/stui/internal/ui/components/poster"
	"github.com/stui/stui/pkg/log"
)

func bingeTickCmd() tea.Cmd {
	return tea.Tick(time.Second, func(time.Time) tea.Msg { return bingeTickMsg{} })
}

func syncHideCmd() tea.Cmd {
	return tea.Tick(syncOverlayDuration, func(time.Time) tea.Msg { return syncHideMsg{} })
}

func mpdElapsedTickCmd() tea.Cmd {
	return tea.Tick(time.Second, func(time.Time) tea.Msg { return mpdElapsedTickMsg{} })
}

func (m Model) Init() tea.Cmd {
	if m.opts.NoRuntime {
		return tea.Batch(
			m.loadingSpinner.Tick,
			func() tea.Msg { return ipc.RuntimeReadyMsg{} },
			pollPosterRefresh(),
			components.ChafaPollCmd(),
		)
	}

	return tea.Batch(
		m.loadingSpinner.Tick,
		func() tea.Msg {
			client, err := ipc.Start(m.opts.RuntimePath)
			if err != nil {
				return ipc.RuntimeErrorMsg{Err: err}
			}
			return runtimeStartedMsg{client: client}
		},
		pollPosterRefresh(),
		components.ChafaPollCmd(),
	)
}

// fromIPC wraps a message that arrived via the IPC channel so that the
// Update switch can re-subscribe listenIPC in a single place.
type fromIPC struct{ tea.Msg }

// UnwrapIPC returns the wrapped IPC message and true if msg is a fromIPC
// envelope. Used by the splash screen wrapper (cmd/stui/main.go) to peek
// at IPC traffic for progress-bar milestones without consuming the message
// — the inner Model still needs to handle it.
func UnwrapIPC(msg tea.Msg) (tea.Msg, bool) {
	if w, ok := msg.(fromIPC); ok {
		return w.Msg, true
	}
	return msg, false
}

// listenIPC returns a Cmd that blocks on the IPC message channel and
// delivers the next message as a fromIPC wrapper.  Update re-subscribes
// by returning another listenIPC after processing each message.
func listenIPC(ch <-chan tea.Msg) tea.Cmd {
	log.Info("ui: listenIPC cmd built (not yet running)")
	return func() tea.Msg {
		log.Info("ui: listenIPC cmd RUNNING — waiting for message")
		msg, ok := <-ch
		if !ok {
			log.Warn("ui: listenIPC channel closed")
			return fromIPC{ipc.RuntimeErrorMsg{Err: fmt.Errorf("IPC channel closed")}}
		}
		log.Info("ui: listenIPC got message", "type", fmt.Sprintf("%T", msg))
		return fromIPC{msg}
	}
}

// pollPosterRefresh adapts the poster package's Bubbletea-agnostic
// PollRefresh (a func() any) into a tea.Cmd. It's re-armed from both
// Init and the Update handler for PostersUpdatedMsg.
func pollPosterRefresh() tea.Cmd {
	return func() tea.Msg {
		return poster.PollRefresh()()
	}
}

type runtimeStartedMsg struct{ client *ipc.Client }
