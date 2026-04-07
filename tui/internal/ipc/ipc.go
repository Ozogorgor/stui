// Package ipc implements the Go side of the stui IPC bridge.
//
// Transport: newline-delimited JSON over the stdin/stdout of a
// stui-runtime child process.
//
//	Go TUI  ──(Request \n)──▶  stui-runtime
//	Go TUI  ◀──(Response \n)── stui-runtime
//
// Usage:
//
//	client, err := ipc.Start("/usr/local/bin/stui-runtime")
//	defer client.Stop()
//
//	// Send a request and get a response channel
//	ch := client.Send(ipc.SearchRequest{...})
//	resp := <-ch
package ipc

import (
	"bufio"
	"context"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"sync"
	"sync/atomic"

	tea "charm.land/bubbletea/v2"

	"github.com/stui/stui/pkg/log"
)

// Client manages the stui-runtime child process and all IPC with it.
type Client struct {
	cmd       *exec.Cmd
	stdin     io.WriteCloser
	stdout    *bufio.Scanner
	stderrLog *os.File // runtime stderr log file, may be nil

	mu       sync.Mutex
	pending  map[string]chan RawResponse
	reqIDSeq atomic.Uint64

	// out receives every message the client wants to deliver to the UI.
	// The UI drains it via a listenIPC tea.Cmd rather than via program.Send.
	out chan tea.Msg

	ctx    context.Context
	cancel context.CancelFunc
	once   sync.Once

	RuntimeVersion       string
	NegotiatedIPCVersion uint32

	logger *log.IPCLogger
}

// send delivers msg to the UI event loop.
// It is non-blocking: if the channel is full the message is dropped and a
// warning is logged.  The channel is generously buffered (256 slots) so
// this should only occur under extreme load.
func (c *Client) send(msg tea.Msg) {
	select {
	case c.out <- msg:
	default:
		c.logger.Warn("IPC message channel full — dropping message",
			"type", fmt.Sprintf("%T", msg))
	}
}

// Chan returns the read end of the outbound message channel.
// The UI model should call listenIPC(client.Chan()) once after startup
// and re-subscribe after every message to keep the pipeline alive.
func (c *Client) Chan() <-chan tea.Msg { return c.out }

// runtimeLogPath returns the path for the runtime stderr log file.
func runtimeLogPath() string {
	if dir, err := os.UserConfigDir(); err == nil {
		return filepath.Join(dir, "stui", "runtime.log")
	}
	if home, err := os.UserHomeDir(); err == nil {
		return filepath.Join(home, ".config", "stui", "runtime.log")
	}
	return ""
}

// Start spawns the stui-runtime binary and performs a handshake ping.
// The caller should call client.Chan() and drain it via a listenIPC Cmd
// rather than passing a *tea.Program.
func Start(runtimePath string) (*Client, error) {
	ctx, cancel := context.WithCancel(context.Background())

	cmd := exec.CommandContext(ctx, runtimePath)

	// Redirect runtime stderr to a log file so it doesn't bleed into the TUI.
	// Fall back to os.Stderr if the log file can't be opened.
	var stderrLog *os.File
	if logPath := runtimeLogPath(); logPath != "" {
		_ = os.MkdirAll(filepath.Dir(logPath), 0o755)
		if f, err := os.OpenFile(logPath, os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0o644); err == nil {
			cmd.Stderr = f
			stderrLog = f
		} else {
			cmd.Stderr = os.Stderr
		}
	} else {
		cmd.Stderr = os.Stderr
	}

	stdin, err := cmd.StdinPipe()
	if err != nil {
		cancel()
		return nil, fmt.Errorf("ipc: stdin pipe: %w", err)
	}

	stdoutPipe, err := cmd.StdoutPipe()
	if err != nil {
		cancel()
		return nil, fmt.Errorf("ipc: stdout pipe: %w", err)
	}

	if err := cmd.Start(); err != nil {
		cancel()
		return nil, fmt.Errorf("ipc: start runtime: %w", err)
	}

	logger := log.NewIPCLogger().With("runtime_path", runtimePath)
	logger.Info("starting runtime process")

	c := &Client{
		cmd:       cmd,
		stdin:     stdin,
		stdout:    bufio.NewScanner(stdoutPipe),
		stderrLog: stderrLog,
		pending:   make(map[string]chan RawResponse),
		out:       make(chan tea.Msg, 256),
		ctx:       ctx,
		cancel:    cancel,
		logger:    logger,
	}

	go c.readLoop()

	versionOK, err := c.ping()
	if err != nil {
		c.Stop()
		return nil, fmt.Errorf("ipc: handshake ping failed: %w", err)
	}
	logger.Info("handshake completed",
		"runtime_version", c.RuntimeVersion,
		"ipc_version", c.NegotiatedIPCVersion,
		"version_ok", versionOK,
	)
	if !versionOK {
		c.send(IPCVersionMismatchMsg{
			TUIVersion:     IPCVersion,
			RuntimeVersion: c.NegotiatedIPCVersion,
			RuntimeSemver:  c.RuntimeVersion,
		})
	}

	return c, nil
}

// Stop shuts down the runtime process gracefully.
func (c *Client) Stop() {
	c.once.Do(func() {
		c.logger.Info("stopping runtime process")
		_ = c.sendRaw(map[string]any{"type": "shutdown"})
		c.cancel()
		_ = c.stdin.Close()
		_ = c.cmd.Wait()
		if c.stderrLog != nil {
			_ = c.stderrLog.Close()
		}
		c.logger.Info("runtime process stopped")
	})
}
