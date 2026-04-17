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

func (m *SplashScreenModel) Init() tea.Cmd {
	return m.splash.Init()
}

func (m *SplashScreenModel) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	if m.started {
		return m.mainModel.Update(msg)
	}

	switch msg := msg.(type) {
	case tea.WindowSizeMsg:
		// Forward to existing splash so it updates dimensions without re-initializing timers.
		var splashCmd tea.Cmd
		_, splashCmd = m.splash.Update(msg)
		// Forward to main model so it has correct dimensions when the splash finishes.
		// Without this, mainModel.state.Width == 0 on takeover and View() returns
		// "Loading…" without AltScreen=true, causing BubbleTea to exit alt screen.
		var mainCmd tea.Cmd
		m.mainModel, mainCmd = m.mainModel.Update(msg)
		return m, tea.Batch(splashCmd, mainCmd)
	}

	_, cmd := m.splash.Update(msg)
	if m.splash.IsDone() {
		m.started = true
		return m.mainModel, m.mainModel.Init()
	}
	return m, cmd
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
