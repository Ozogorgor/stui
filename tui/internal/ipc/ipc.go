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
	"sync"
	"sync/atomic"

	tea "charm.land/bubbletea/v2"

	"github.com/stui/stui/pkg/log"
)

// Client manages the stui-runtime child process and all IPC with it.
type Client struct {
	cmd    *exec.Cmd
	stdin  io.WriteCloser
	stdout *bufio.Scanner

	mu       sync.Mutex
	pending  map[string]chan RawResponse
	reqIDSeq atomic.Uint64

	program *tea.Program
	ctx     context.Context
	cancel  context.CancelFunc
	once    sync.Once

	RuntimeVersion       string
	NegotiatedIPCVersion uint32

	logger *log.IPCLogger
}

// Start spawns the stui-runtime binary and performs a handshake ping.
func Start(runtimePath string, program *tea.Program) (*Client, error) {
	ctx, cancel := context.WithCancel(context.Background())

	cmd := exec.CommandContext(ctx, runtimePath)
	cmd.Stderr = os.Stderr

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
		cmd:     cmd,
		stdin:   stdin,
		stdout:  bufio.NewScanner(stdoutPipe),
		pending: make(map[string]chan RawResponse),
		program: program,
		ctx:     ctx,
		cancel:  cancel,
		logger:  logger,
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
	if !versionOK && program != nil {
		program.Send(IPCVersionMismatchMsg{
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
		c.logger.Info("runtime process stopped")
	})
}
