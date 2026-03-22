# Contributing to stui

Thank you for your interest in contributing to stui! This document provides guidelines and instructions for contributing.

## Code of Conduct

- Be respectful and constructive in all interactions
- Focus on what's best for the project and community
- Keep discussions technical and on-topic
- Welcome diverse perspectives and experience levels

## Getting Started

1. **Fork the repository** on GitHub
2. **Clone your fork** locally:
   ```bash
   git clone https://github.com/your-username/stui.git
   cd stui
   ```
3. **Set up your development environment** (see [DEVELOPER_SETUP.md](DEVELOPER_SETUP.md))
4. **Create a branch** for your changes:
   ```bash
   git checkout -b feature/your-feature-name
   # or
   git checkout -b fix/your-bug-fix
   ```

## Development Workflow

### 1. Before Writing Code

- Check existing issues and PRs to avoid duplication
- For significant changes, open an issue first to discuss the approach
- For bug fixes, add a test that reproduces the issue

### 2. While Developing

- Follow the existing code style and conventions
- Write tests for new functionality
- Keep commits focused and atomic
- Update documentation as needed

### 3. Before Submitting

```bash
# Run all tests
./scripts/test.sh

# Run code quality checks
./scripts/check.sh

# Build to ensure compilation succeeds
./scripts/build.sh
```

## Pull Request Process

### Creating a PR

1. **Title**: Clear and descriptive
   - Good: `feat: add HTTP request timeouts to metadata providers`
   - Bad: `fix stuff`, `update code`

2. **Description**: Explain what and why
   - Reference any related issues with `Fixes #123` or `Closes #123`
   - Describe the changes made
   - Note any breaking changes

3. **Size**: Keep PRs manageable
   - Break large changes into smaller, logical PRs
   - A PR should be reviewable in 15-20 minutes

### PR Template

```markdown
## Summary
Brief description of the changes.

## Motivation
Why is this change needed? What problem does it solve?

## Changes
- Change 1
- Change 2

## Testing
How was this tested? Any manual testing steps?

## Checklist
- [ ] Tests pass
- [ ] Code is formatted
- [ ] Documentation updated (if applicable)
- [ ] No new warnings introduced
```

### Review Process

1. Automated checks must pass (CI/CD)
2. At least one maintainer review required
3. Address feedback promptly or explain delays
4. Squash commits before merging if requested

## Code Style

### Go

Follow these conventions (enforced by golangci-lint):

```go
// Package naming: short, lowercase
package ipc

// Variable naming: camelCase for locals, PascalCase for exports
var pendingRequests int
const MaxRetries = 3

// Error handling: wrap errors with context
if err != nil {
    return fmt.Errorf("fetch metadata: %w", err)
}

// Comments: doc comments for exported functions
// Send sends a request to the runtime and waits for response.
func (c *Client) Send(req Request) Response {
```

Run formatting:
```bash
cd tui && gofmt -w . && golangci-lint run
```

### Rust

Follow these conventions (enforced by clippy):

```rust
// Module-level documentation
//! The IPC module handles communication with the runtime.

// Struct naming
pub struct ConfigManager { ... }

// Error handling with thiserror
#[derive(Error, Debug)]
pub enum Error {
    #[error("connection failed: {0}")]
    Connection(String),
}

// Documentation for public items
/// Creates a new client connected to the runtime.
pub fn new() -> Client { ... }
```

Run formatting:
```bash
cargo fmt
cargo clippy --workspace -- -D warnings
```

## Testing

### Go Tests

```bash
cd tui

# Run all tests
go test ./...

# Run tests with coverage
go test -cover ./...

# Run specific test
go test -v ./internal/ipc/... -run TestSearch

# Run integration tests
go test -tags=integration ./...
```

### Rust Tests

```bash
# Unit tests
cargo test -p stui-runtime

# Integration tests
cargo test -p stui-runtime --tests

# All tests
cargo test --workspace

# With output
cargo test -- --nocapture
```

### Test Coverage Goals

| Area | Target Coverage |
|------|-----------------|
| Core business logic | 80%+ |
| IPC layer | 90%+ |
| Provider abstraction | 70%+ |
| UI state management | 50%+ |

## Commit Messages

### Format

```
<type>(<scope>): <subject>

<body>

<footer>
```

### Types

| Type | Description |
|------|-------------|
| feat | New feature |
| fix | Bug fix |
| docs | Documentation changes |
| style | Formatting, no code change |
| refactor | Code restructuring |
| test | Adding/updating tests |
| chore | Maintenance, deps, build |

### Examples

```
feat(ipc): add request timeout support

Add configurable timeouts to all HTTP requests to prevent
indefinite hangs when providers are unresponsive.

Fixes #42

---

fix(provider): handle rate limit responses correctly

Return proper retry-after delay instead of immediately retrying.
Exponential backoff now implemented.

---

docs(readme): update installation instructions

Add steps for Debian/Ubuntu package installation.
```

## Reporting Issues

### Bug Reports

Include:
- stui version (`stui --version` or git commit)
- Rust runtime version
- Go version
- Distribution and version
- Steps to reproduce
- Expected vs actual behavior
- Relevant logs (`RUST_LOG=debug stui 2>&1 | head -100`)

### Feature Requests

Include:
- Clear description of the feature
- Use case / motivation
- Potential alternatives considered
- Any implementation ideas

## Project Architecture

See [ARCHITECTURE.md](ARCHITECTURE.md) for high-level design.

Key concepts:
- **IPC**: Go TUI communicates with Rust runtime via NDJSON over stdin/stdout
- **Providers**: Plugin trait for search, streams, subtitles, metadata
- **Pipeline**: Orchestrates search → resolve → rank → play flow
- **Events**: EventBus for loose coupling between components

## Questions?

- Open an issue for bugs or feature requests
- Check existing issues before creating new ones
- Be patient — maintainers respond when able

## License

By contributing, you agree that your contributions will be licensed under the same license as the project.
