//! `stui plugin load <dir>` — fast iteration entry point for plugin
//! authors. Parses + validates the manifest and resolves the declared
//! entrypoint (without spawning a wasmtime instance or the supervisor),
//! then prints a summary of what the runtime would see.
//!
//! Unlike `plugin lint` (which only checks the manifest), `load` also
//! verifies the entrypoint file exists on disk and reports its execution
//! mode. Use this when iterating on a plugin's manifest + build outputs
//! — it answers "would the runtime accept this?" in milliseconds, no
//! WASM cost.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use stui_plugin_sdk::PluginManifest;

/// Execution mode inferred from the entrypoint suffix. Mirrors the
/// runtime's `plugin::loader::ExecutionMode` minus the gRPC variant
/// (gRPC plugins don't have a local file to verify).
#[derive(Debug)]
enum EntrypointMode {
    Wasm,
    NativeLib,
    /// Tagged for parity with the runtime's loader; the URL itself isn't
    /// consumed by the CLI summary — only the variant determines whether
    /// we skip the on-disk check.
    Grpc(#[allow(dead_code)] String),
}

pub fn run(dir: PathBuf) -> Result<()> {
    let dir = dir
        .canonicalize()
        .with_context(|| format!("plugin directory not found: {}", dir.display()))?;
    let manifest_path = dir.join("plugin.toml");
    if !manifest_path.exists() {
        anyhow::bail!("no plugin.toml in {}", dir.display());
    }

    let raw = std::fs::read_to_string(&manifest_path).context("read plugin.toml")?;
    let manifest: PluginManifest = toml::from_str(&raw)
        .context("plugin.toml is not valid TOML against PluginManifest schema")?;

    // Reuse the lint pipeline — same validation the runtime applies.
    crate::cmd::lint::run_manifest(&manifest)?;

    // Resolve and verify the entrypoint.
    let (mode, entrypoint) = resolve_entrypoint(&dir, &manifest)?;
    if let EntrypointMode::Wasm | EntrypointMode::NativeLib = &mode {
        if !entrypoint.exists() {
            anyhow::bail!(
                "entrypoint missing: {}\n  (declared in plugin.toml as `entrypoint = \"{}\"`; run `stui plugin build` first)",
                entrypoint.display(),
                manifest.plugin.entrypoint,
            );
        }
    }

    print_summary(&manifest, &mode, &entrypoint);
    Ok(())
}

fn resolve_entrypoint(dir: &Path, manifest: &PluginManifest) -> Result<(EntrypointMode, PathBuf)> {
    let entry = &manifest.plugin.entrypoint;
    if entry.starts_with("grpc://") {
        return Ok((EntrypointMode::Grpc(entry.clone()), PathBuf::from(entry)));
    }
    let abs = dir.join(entry);
    let mode = if entry.ends_with(".wasm") {
        EntrypointMode::Wasm
    } else if entry.ends_with(".so") || entry.ends_with(".dylib") || entry.ends_with(".dll") {
        EntrypointMode::NativeLib
    } else {
        anyhow::bail!(
            "unknown entrypoint format: {} (expected .wasm / .so / .dylib / .dll / grpc://)",
            entry,
        );
    };
    Ok((mode, abs))
}

fn print_summary(manifest: &PluginManifest, mode: &EntrypointMode, entrypoint: &Path) {
    println!("Plugin loaded ✓");
    println!("  name        : {}", manifest.plugin.name);
    println!("  version     : {}", manifest.plugin.version);
    println!(
        "  entrypoint  : {} ({})",
        entrypoint.display(),
        describe_mode(mode)
    );
    let cap = &manifest.capabilities;
    let mut decls = Vec::new();
    let catalog_active = match &cap.catalog {
        stui_plugin_sdk::CatalogCapability::Enabled(b) => *b,
        stui_plugin_sdk::CatalogCapability::Typed { search, kinds, .. } => {
            search.unwrap_or(false) || !kinds.is_empty()
        }
    };
    if catalog_active {
        decls.push("catalog");
    }
    if cap.streams {
        decls.push("streams");
    }
    println!(
        "  capabilities: {}",
        if decls.is_empty() {
            "(none)".to_string()
        } else {
            decls.join(", ")
        },
    );
}

fn describe_mode(mode: &EntrypointMode) -> &'static str {
    match mode {
        EntrypointMode::Wasm => "wasm",
        EntrypointMode::NativeLib => "native-lib",
        EntrypointMode::Grpc(_) => "grpc",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn write_plugin(dir: &Path, manifest_toml: &str, with_wasm: bool) {
        fs::write(dir.join("plugin.toml"), manifest_toml).unwrap();
        if with_wasm {
            fs::write(dir.join("plugin.wasm"), b"\0asm\x01\0\0\0").unwrap();
        }
    }

    #[test]
    fn load_succeeds_when_manifest_valid_and_entrypoint_exists() {
        let dir = tempdir().unwrap();
        write_plugin(
            dir.path(),
            r#"
[plugin]
id         = "test"
name       = "test"
version    = "0.1.0"
entrypoint = "plugin.wasm"

[capabilities.catalog]
kinds  = ["movie"]
search = true
"#,
            true,
        );
        run(dir.path().to_path_buf()).expect("should load cleanly");
    }

    #[test]
    fn load_fails_when_entrypoint_missing() {
        let dir = tempdir().unwrap();
        write_plugin(
            dir.path(),
            r#"
[plugin]
id         = "test"
name       = "test"
version    = "0.1.0"
entrypoint = "plugin.wasm"

[capabilities.catalog]
kinds  = ["movie"]
search = true
"#,
            false,
        );
        let err = run(dir.path().to_path_buf()).unwrap_err();
        assert!(err.to_string().contains("entrypoint missing"), "{err}");
    }

    #[test]
    fn load_fails_when_no_plugin_toml() {
        let dir = tempdir().unwrap();
        let err = run(dir.path().to_path_buf()).unwrap_err();
        assert!(err.to_string().contains("no plugin.toml"), "{err}");
    }

    #[test]
    fn load_fails_on_invalid_manifest() {
        let dir = tempdir().unwrap();
        write_plugin(
            dir.path(),
            r#"
[plugin]
id      = "bad"
name    = "bad"
version = "0.1.0"
entrypoint = "plugin.wasm"

[capabilities.catalog]
kinds = ["movie"]
# missing search = true → manifest validation fails
"#,
            true,
        );
        let err = run(dir.path().to_path_buf()).unwrap_err();
        assert!(
            err.to_string().contains("manifest validation failed"),
            "{err}"
        );
    }

    #[test]
    fn load_recognizes_grpc_entrypoint_without_file_check() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("plugin.toml"),
            r#"
[plugin]
id         = "rpc-plugin"
name       = "rpc-plugin"
version    = "0.1.0"
entrypoint = "grpc://localhost:50051"

[capabilities.catalog]
kinds  = ["movie"]
search = true
"#,
        )
        .unwrap();
        run(dir.path().to_path_buf()).expect("grpc plugins skip on-disk check");
    }
}
