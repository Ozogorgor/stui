# Developer Setup Guide

This guide covers setting up a local development environment for stui.

## Prerequisites

### Required Dependencies

| Dependency | Version | Purpose |
|------------|---------|---------|
| Go | 1.22+ | TUI frontend |
| Rust | 1.75+ | Runtime backend |
| mpv | latest | Media playback |
| aria2c | latest | Torrent download |
| git | latest | Version control |

### Optional Dependencies

| Dependency | Purpose |
|------------|---------|
| python3 | RPC plugin development |
| wasm-pack | WASM plugin compilation |
| golangci-lint | Go linting |
| cargo-audit | Security vulnerability scanning |

### System Packages (Debian/Ubuntu)

```bash
sudo apt install -y \
    build-essential \
    curl \
    git \
    golang-go \
    mpv \
    aria2 \
    pkg-config \
    libssl-dev
```

### Rust Installation

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env
rustup default stable
```

### Go Installation (if not using system Go)

```bash
wget https://go.dev/dl/go1.22.0.linux-amd64.tar.gz
sudo tar -C /usr/local -xzf go1.22.0.linux-amd64.tar.gz
export PATH=$PATH:/usr/local/go/bin
```

## Repository Setup

```bash
# Clone the repository
git clone https://github.com/your-repo/stui.git
cd stui

# Install Go dependencies
cd tui && go mod download && cd ..

# Install Rust dependencies
cargo fetch
```

## Building

### Build Everything

```bash
./scripts/build.sh
```

This produces:
- `dist/stui` - Main binary
- `dist/stui-runtime` - Runtime binary
- `dist/libstui-sdk.{so,a}` - SDK library

### Build Individual Components

```bash
# Go TUI only
cd tui && go build -o ../dist/stui ./cmd/stui

# Rust runtime only
cargo build --release -p stui-runtime
cp target/release/stui-runtime dist/

# All Rust workspace
cargo build --release --workspace
```

## Development Workflow

### Running Locally

```bash
# Start the TUI with runtime auto-spawned
./dist/stui

# Or run TUI without runtime (UI-only mode for development)
./dist/stui -no-runtime

# Verbose logging
./dist/stui -v

# JSON log output (for log aggregation)
./dist/stui -json
```

### Running Tests

```bash
# All tests (Go + Rust)
./scripts/test.sh

# Go tests only
cd tui && go test ./...

# Rust tests only
cargo test --workspace

# Rust integration tests
cargo test -p stui-runtime --tests

# Go with coverage
cd tui && go test -cover ./...
```

### Code Quality Checks

```bash
# Run all checks (Go linting, Rust linting, security audit)
./scripts/check.sh

# Go linting only
cd tui && golangci-lint run

# Rust linting only
cargo clippy --workspace

# Security audit
cargo audit
```

### Development Helpers

```bash
# Start aria2c daemon (required for torrent streaming)
./scripts/aria2c-start.sh

# Watch mode for Go (requires fswatch)
# cd tui && fswatch -r . | xargs -I{} sh -c 'go build ./...'

# Format code
cargo fmt
cd tui && gofmt -w .
```

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `STUI_RUNTIME` | auto-detect | Path to stui-runtime binary |
| `STUI_CONFIG` | `~/.config/stui` | Config directory |
| `STUI_PLUGINS` | `~/.stui/plugins` | Plugin directory |
| `RUST_LOG` | `info` | Rust logging level |
| `RUST_BACKTRACE` | `1` | Enable backtraces |

### API Keys

For full functionality, set these environment variables or add them to `~/.stui/secrets.env`:

```bash
export TMDB_API_KEY=your_tmdb_key
export OMDB_API_KEY=your_omdb_key
export LASTFM_API_KEY=your_lastfm_key
export OPENSUBTITLES_API_KEY=your_os_key
export PROWLARR_API_KEY=your_prowlarr_key
export ARIA2_SECRET=your_aria2_secret
```

## Project Structure

```
stui/
├── tui/                    # Go TUI (BubbleTea)
│   ├── cmd/stui/          # Entry point
│   ├── internal/
│   │   ├── ipc/           # IPC client
│   │   ├── ui/            # UI components
│   │   └── state/         # App state
│   └── pkg/               # Shared packages
│       ├── log/           # Logging
│       ├── theme/         # Styling
│       └── ...
├── runtime/               # Rust runtime (Tokio)
│   ├── src/
│   │   ├── main.rs        # Entry point
│   │   ├── engine/        # Pipeline orchestration
│   │   ├── providers/     # Built-in providers
│   │   ├── player/        # mpv integration
│   │   └── config/        # Configuration
│   └── tests/             # Integration tests
├── plugins/               # Official plugins
│   └── torrentio-rpc/     # Torrentio RPC plugin
├── docs/                  # Architecture docs
├── scripts/               # Build/test scripts
└── config/                # Default config files
```

## IDE Setup

### VS Code

Recommended extensions:
- `rust-lang.rust-analyzer` - Rust LSP
- `golang.go` - Go support
- `mskelton.npm-outdated` - Dependency updates

Settings (`~/.config/Code/User/settings.json`):
```json
{
  "rust-analyzer.checkOnSave.command": "clippy",
  "go.lintTool": "golangci-lint",
  "go.lintOnSave": true
}
```

### Neovim

```lua
-- Using lazy.nvim
{
  'neovim/nvim-lspconfig',
  dependencies = {
    'simrat39/rust-tools.nvim',
    'mrcjkb/rustaceanvim',
  },
  config = function()
    local lsp = require('lspconfig')
    lsp.golang.setup({})
    lsp.rust_analyzer.setup({})
  end
}
```

## Common Issues

### "cannot find -lssl"

```bash
sudo apt install libssl-dev pkg-config
```

### "mpv not found"

```bash
sudo apt install mpv
# Or build from source for latest features
```

### Go build fails with "module not found"

```bash
cd tui && go mod tidy && go mod download
```

### Rust compilation errors

```bash
# Update toolchain
rustup update stable

# Clean build artifacts
cargo clean && cargo build
```

### IPC connection failures

Ensure `stui-runtime` is in your PATH or set `STUI_RUNTIME`:

```bash
export STUI_RUNTIME=/path/to/stui-runtime
```

## Next Steps

- Read [ARCHITECTURE.md](ARCHITECTURE.md) for system design
- Read [runtime-ipc.md](runtime-ipc.md) for IPC protocol
- Read [plugins.md](plugins.md) for plugin development
- Check [CONTRIBUTING.md](CONTRIBUTING.md) for contribution guidelines
