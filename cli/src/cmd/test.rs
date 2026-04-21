use anyhow::{Context, Result};

/// Run `cargo test` inside the current plugin directory.
///
/// This is a thin wrapper: the plugin's own test suite is responsible for
/// setting up a mocked host (using the SDK's test utilities) and exercising
/// the plugin's verb implementations.  The CLI simply spawns `cargo test` and
/// propagates the exit code.
pub fn run() -> Result<()> {
    let status = std::process::Command::new("cargo")
        .arg("test")
        .status()
        .context("cargo test failed to spawn")?;
    if !status.success() {
        anyhow::bail!("cargo test exited non-zero");
    }
    Ok(())
}
