// handlers_downloads.go — Update msg handlers for the offline
// downloads feature plus its currentDownloads helper. Each handler
// is the body of a case in Update; ui.go's Update switch dispatches
// to them via `return m.handleXxx(msg)` lines.

package ui

import (
	tea "charm.land/bubbletea/v2"

	"github.com/stui/stui/internal/ipc"
	"github.com/stui/stui/pkg/notify"
)

// handleDownloadStarted handles ipc.DownloadStartedMsg.
func (m Model) handleDownloadStarted(msg ipc.DownloadStartedMsg) (tea.Model, tea.Cmd) {
	if _, exists := m.downloadMap[msg.GID]; !exists {
		m.downloadOrder = append(m.downloadOrder, msg.GID)
	}
	title := msg.Title
	if title == "" {
		title = msg.URI
	}
	m.downloadMap[msg.GID] = &ipc.DownloadEntry{
		GID:    msg.GID,
		Title:  title,
		Status: "active",
	}
	return m, nil
}

// handleDownloadProgress handles ipc.DownloadProgressMsg.
func (m Model) handleDownloadProgress(msg ipc.DownloadProgressMsg) (tea.Model, tea.Cmd) {
	if e, ok := m.downloadMap[msg.GID]; ok {
		e.Progress = msg.Progress
		e.Speed = msg.Speed
		e.ETA = msg.ETA
		e.Seeders = msg.Seeders
	}
	return m, nil
}

// handleDownloadComplete handles ipc.DownloadCompleteMsg.
func (m Model) handleDownloadComplete(msg ipc.DownloadCompleteMsg) (tea.Model, tea.Cmd) {
	if e, ok := m.downloadMap[msg.GID]; ok {
		e.Status = "complete"
		e.Progress = 1.0
		e.Files = msg.Files
		e.Speed = ""
		e.ETA = ""
		if m.notifyCfg.OnDownload {
			title := e.Title
			if title == "" {
				title = msg.GID
			}
			notify.Send(m.notifyCfg, "✓ Download Complete", title, notify.UrgencyNormal)
		}
	}
	return m, nil
}

// handleDownloadError handles ipc.DownloadErrorMsg.
func (m Model) handleDownloadError(msg ipc.DownloadErrorMsg) (tea.Model, tea.Cmd) {
	if e, ok := m.downloadMap[msg.GID]; ok {
		e.Status = "error"
		e.Error = msg.Message
	}
	return m, nil
}

// currentDownloads returns a snapshot of the download list in arrival order.
func (m *Model) currentDownloads() []*ipc.DownloadEntry {
	out := make([]*ipc.DownloadEntry, 0, len(m.downloadOrder))
	for _, gid := range m.downloadOrder {
		if e, ok := m.downloadMap[gid]; ok {
			cp := *e
			out = append(out, &cp)
		}
	}
	return out
}
