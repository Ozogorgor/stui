package main

import (
	"flag"
	"fmt"
	"os"
	"path/filepath"

	tea "charm.land/bubbletea/v2"

	"github.com/stui/stui/internal/ui"
	"github.com/stui/stui/internal/ui/components"
	"github.com/stui/stui/pkg/log"
)

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
		m.splash = components.NewSplash(msg.Width, msg.Height)
		return m, m.splash.Init()
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

	opts := ui.Options{
		RuntimePath: *runtimePath,
		NoRuntime:   *noRuntime,
		Verbose:     *verbose,
	}

	mainModel := ui.New(opts)

	if *noSplash {
		p := tea.NewProgram(mainModel)
		mainModel.SetProgram(p)
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

	p := tea.NewProgram(model)
	mainModel.SetProgram(p)

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
