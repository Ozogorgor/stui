package main

import (
	"flag"
	"fmt"
	"net"
	"os"
	"os/signal"
	"path/filepath"
	"syscall"
	"time"

	tea "charm.land/bubbletea/v2"

	"github.com/stui/stui/internal/ipc"
	"github.com/stui/stui/internal/ui"
	"github.com/stui/stui/internal/ui/components"
	"github.com/stui/stui/pkg/config"
	"github.com/stui/stui/pkg/log"
	"github.com/stui/stui/pkg/theme"
)

// tuiLogPath returns ~/.config/stui/tui.log (same dir as runtime.log).
func tuiLogPath() string {
	if dir, err := os.UserConfigDir(); err == nil {
		return filepath.Join(dir, "stui", "tui.log")
	}
	if home, err := os.UserHomeDir(); err == nil {
		return filepath.Join(home, ".config", "stui", "tui.log")
	}
	return ""
}

type SplashScreenModel struct {
	splash    *components.Splash
	mainModel tea.Model
	started   bool
}

// Init kicks off BOTH the splash animation and the main model in parallel.
// Running mainModel.Init() here (instead of waiting for the splash to finish)
// lets the runtime handshake, plugin discovery, and initial grid load happen
// behind the splash — turning it from pure eye-candy into a real loading
// indicator backed by the progress bar at the bottom.
func (m *SplashScreenModel) Init() tea.Cmd {
	return tea.Batch(m.splash.Init(), m.mainModel.Init())
}

func (m *SplashScreenModel) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	if m.started {
		return m.mainModel.Update(msg)
	}

	// Tap milestone messages BEFORE forwarding so the splash progress
	// bar advances as the runtime, plugins, and first grid arrive.
	// We don't consume the messages — the main model still needs them
	// to populate its own state.
	//
	// IPC messages reach this layer wrapped in a `fromIPC` envelope
	// (see ui/init.go) — the inner Model unwraps them on its end.
	// We use the public `ui.UnwrapIPC` helper to peek inside without
	// consuming the envelope.
	var milestoneCmd tea.Cmd
	inner := msg
	if unwrapped, ok := ui.UnwrapIPC(msg); ok {
		inner = unwrapped
	}
	switch ev := inner.(type) {
	case ipc.RuntimeReadyMsg:
		milestoneCmd = m.splash.MarkRuntimeReady()
	case ipc.PluginListMsg:
		milestoneCmd = m.splash.MarkPluginsLoaded()
	case ipc.GridUpdateMsg:
		milestoneCmd = m.splash.MarkGridReady(ev.Tab)
	}

	// WindowSizeMsg needs to reach both: the splash uses it for centering
	// and the main model needs it before takeover so View() doesn't render
	// without AltScreen=true and bounce out of alt screen.
	if _, ok := msg.(tea.WindowSizeMsg); ok {
		_, splashCmd := m.splash.Update(msg)
		var mainCmd tea.Cmd
		m.mainModel, mainCmd = m.mainModel.Update(msg)
		return m, tea.Batch(splashCmd, mainCmd, milestoneCmd)
	}

	// Drive the splash (animation tick + progress.FrameMsg) AND the main
	// model in parallel during the splash phase. The main model's IPC
	// pumps, plugin loaders, and grid hydration all run while the
	// animation plays — when the splash dismisses, the user is dropped
	// into a fully-warm UI instead of a "Loading…" placeholder.
	_, splashCmd := m.splash.Update(msg)
	var mainCmd tea.Cmd
	m.mainModel, mainCmd = m.mainModel.Update(msg)

	if m.splash.IsDone() {
		m.started = true
		// mainModel.Init() already ran from our Init(); don't re-call
		// it or its IPC pumps will be subscribed twice. Just emit any
		// pending Cmds from this turn and let the main model take over
		// on the next message.
		return m.mainModel, tea.Batch(splashCmd, mainCmd, milestoneCmd)
	}
	return m, tea.Batch(splashCmd, mainCmd, milestoneCmd)
}

func (m *SplashScreenModel) View() tea.View {
	if m.started {
		return m.mainModel.View()
	}
	return m.splash.View()
}

func main() {
	runtimePath := flag.String(
		"runtime",
		defaultRuntimePath(),
		"path to stui-runtime binary",
	)
	noRuntime := flag.Bool(
		"no-runtime",
		false,
		"start without the Rust runtime (UI-only mode, useful for development)",
	)
	verbose := flag.Bool("v", false, "enable verbose (debug) logging")
	jsonLog := flag.Bool("json", false, "output logs in JSON format")
	noSplash := flag.Bool("no-splash", false, "skip the splash screen on startup")
	flag.Parse()

	// Redirect TUI logs to a file so they don't bleed into the terminal.
	// This runs before any log calls so nothing is lost.
	var logFile *os.File
	if lp := tuiLogPath(); lp != "" {
		_ = os.MkdirAll(filepath.Dir(lp), 0o755)
		if f, err := os.OpenFile(lp, os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0o644); err == nil {
			logFile = f
			log.SetOutput(f)
		}
	}
	defer func() {
		if logFile != nil {
			logFile.Close()
		}
	}()

	if *verbose {
		log.SetLevel(log.LevelDebug)
	}
	if *jsonLog {
		log.Setup(&log.Config{Level: log.LevelInfo, Format: log.FormatJSON})
	}

	log.Info("starting stui",
		"runtime_path", *runtimePath,
		"no_runtime", *noRuntime,
		"version", version(),
	)

	cfgPath := config.DefaultPath()
	// First-launch bootstrap: write a populated config.toml + the
	// bundled starter theme files if either is missing. Both are
	// idempotent — an existing file or non-empty themes/ dir is
	// left alone — so user edits and deletions persist across
	// launches.
	if err := config.EnsureExists(cfgPath); err != nil {
		log.Warn("failed to write default config", "path", cfgPath, "error", err)
	}
	if err := config.EnsureBundledThemes(); err != nil {
		log.Warn("failed to write bundled themes", "error", err)
	}
	cfg, err := config.Load(cfgPath)
	if err != nil {
		log.Warn("failed to load config, using defaults", "error", err)
		cfg = config.Default()
	}

	// Apply the configured theme before the UI starts.
	if cfg.Interface.Theme != "matugen" {
		if palette, err := config.LoadTheme(cfg.Interface.Theme); err == nil {
			theme.T.Apply(palette)
		}
	}

	// Catch SIGTERM/SIGHUP (WM window close, kill, etc.) and stop MPD
	// playback before dying. Uses a raw TCP connection to MPD — faster and
	// more reliable than routing through the IPC client during shutdown.
	mpdAddr := fmt.Sprintf("%s:%d", cfg.MPD.Host, cfg.MPD.Port)
	sigCh := make(chan os.Signal, 1)
	signal.Notify(sigCh, syscall.SIGTERM, syscall.SIGHUP)
	go func() {
		<-sigCh
		conn, err := net.DialTimeout("tcp", mpdAddr, time.Second)
		if err == nil {
			buf := make([]byte, 256)
			conn.Read(buf) // consume MPD greeting
			conn.Write([]byte("stop\n"))
			conn.Close()
		}
		os.Exit(0)
	}()

	opts := ui.Options{
		RuntimePath: *runtimePath,
		NoRuntime:   *noRuntime,
		Verbose:     *verbose,
		CfgPath:     cfgPath,
	}

	// Wrap the inner Model in RootModel so that screen.TransitionMsg (used by
	// settings, search, help, etc.) is intercepted and the active screen is
	// swapped correctly. Without this wrapper all TransitionCmd returns from
	// Model.Update() are silently dropped.
	innerModel := ui.New(opts, cfg)
	mainModel := ui.NewRootModel(ui.NewLegacyScreen(innerModel))

	var p *tea.Program

	cfgWatcher, watchErr := config.NewWatcher(cfgPath, func(c config.Config) {
		if p != nil {
			p.Send(config.ConfigReloadMsg{Config: c})
		}
	})
	if watchErr != nil {
		log.Warn("could not start config watcher", "error", watchErr)
		cfgWatcher = nil
	}
	if cfgWatcher != nil {
		cfgWatcher.SetActiveTheme(cfg.Interface.Theme)
	}
	defer func() {
		if cfgWatcher != nil {
			cfgWatcher.Stop()
		}
	}()

	if *noSplash {
		p = tea.NewProgram(&mainModel)
		mainModel.SetProgram(p)
		if cfgWatcher != nil {
			cfgWatcher.Start()
		}
		if _, err := p.Run(); err != nil {
			log.Error("stui terminated with error", "error", err)
			fmt.Fprintf(os.Stderr, "stui error: %v\n", err)
			os.Exit(1)
		}
		log.Info("stui shutdown complete")
		return
	}

	model := &SplashScreenModel{
		splash:    components.NewSplash(80, 24),
		mainModel: mainModel,
	}

	p = tea.NewProgram(model)
	(&mainModel).SetProgram(p)
	if cfgWatcher != nil {
		cfgWatcher.Start()
	}

	if _, err := p.Run(); err != nil {
		log.Error("stui terminated with error", "error", err)
		fmt.Fprintf(os.Stderr, "stui error: %v\n", err)
		os.Exit(1)
	}

	log.Info("stui shutdown complete")
}

func defaultRuntimePath() string {
	// 1. Check $STUI_RUNTIME env override
	if v := os.Getenv("STUI_RUNTIME"); v != "" {
		return v
	}
	// 2. Assume it lives alongside the stui binary
	exe, err := os.Executable()
	if err != nil {
		return "stui-runtime"
	}
	return filepath.Join(filepath.Dir(exe), "stui-runtime")
}

func version() string {
	// TODO: Build with -ldflags to inject version
	return "dev"
}
