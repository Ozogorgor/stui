use anyhow::{Context, Result};
use std::process::Command;

pub fn run(release: bool) -> Result<()> {
    let cwd = std::env::current_dir()?;

    if !cwd.join("plugin.toml").exists() {
        anyhow::bail!("no plugin.toml in current directory");
    }
    if !cwd.join("Cargo.toml").exists() {
        anyhow::bail!("no Cargo.toml in current directory");
    }

    // ── Step 1: cargo build --target wasm32-wasip1 ───────────────────────────

    let mut cmd = Command::new("cargo");
    cmd.args(["build", "--target", "wasm32-wasip1"]);
    if release {
        cmd.arg("--release");
    }
    let status = cmd.status().context("cargo build failed to spawn")?;
    if !status.success() {
        anyhow::bail!("cargo build exited non-zero");
    }

    // ── Step 2: parse + validate plugin.toml ────────────────────────────────

    let manifest_text =
        std::fs::read_to_string(cwd.join("plugin.toml")).context("read plugin.toml")?;
    let manifest: stui_plugin_sdk::PluginManifest = toml::from_str(&manifest_text)
        .context("plugin.toml is not valid TOML against PluginManifest schema")?;
    stui_plugin_sdk::capabilities::validate_manifest(&manifest)
        .context("plugin.toml failed manifest validation")?;

    // ── Step 3: stub check (--release guard) ─────────────────────────────────
    //
    // TODO(Task 2.4 / future manifest expansion): once CatalogCapability gains
    // per-verb sub-tables with a `stub = true` flag (tracked in Task 1.7 follow-
    // on), replace `has_stubs` with a real implementation.  The current SDK
    // `CatalogCapability` only carries `id_sources: Vec<String>` — there are no
    // verb-config shapes to inspect, so stub detection is not possible from the
    // SDK types today.  `--release` rejection is therefore a no-op until the SDK
    // schema is extended.

    if release && has_stubs(&manifest) {
        anyhow::bail!(
            "--release build rejected: plugin has stubbed verbs. Remove stubs or drop --release."
        );
    }

    // TODO(Task 2.4): call crate::cmd::lint::run() once Task 2.4 lands a real
    // implementation.  Calling it now would always fail with "not yet implemented",
    // so we skip it here and let Task 2.4 wire it back in.

    // ── Step 4: report artifact path ─────────────────────────────────────────

    let profile = if release { "release" } else { "debug" };
    match wasm_artifact_path(&cwd, profile) {
        Some(path) if path.exists() => {
            println!("Build OK. Artifact: {}", path.display());
        }
        _ => {
            println!("Build OK.");
        }
    }

    Ok(())
}

/// Return the expected `.wasm` artifact path by reading `package.name` from the
/// plugin's own `Cargo.toml`.  The crate name is derived by replacing `-` with
/// `_` (cargo's standard library-name mangling).
fn wasm_artifact_path(
    plugin_dir: &std::path::Path,
    profile: &str,
) -> Option<std::path::PathBuf> {
    let cargo_text = std::fs::read_to_string(plugin_dir.join("Cargo.toml")).ok()?;
    let parsed: toml::Value = toml::from_str(&cargo_text).ok()?;
    let package_name = parsed
        .get("package")
        .and_then(|p| p.get("name"))
        .and_then(|n| n.as_str())?
        .replace('-', "_");

    Some(
        plugin_dir
            .join("target")
            .join("wasm32-wasip1")
            .join(profile)
            .join(format!("{package_name}.wasm")),
    )
}

/// Returns `true` if the manifest declares any stubbed verbs.
///
/// NOTE: The current SDK `CatalogCapability` only carries `id_sources:
/// Vec<String>` — it has no per-verb sub-tables, no `stub = true` field, and no
/// `is_stub()` methods.  Until the SDK schema is extended (tracked in the Task
/// 1.7 follow-on), this function always returns `false` and the `--release`
/// rejection gate is effectively disabled.
///
/// TODO(Task 2.4+): implement once `CatalogCapability` gains verb-config types
/// that can express `stub = true`.
fn has_stubs(_manifest: &stui_plugin_sdk::PluginManifest) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_manifest(toml_str: &str) -> stui_plugin_sdk::PluginManifest {
        toml::from_str(toml_str).expect("test manifest should parse")
    }

    #[test]
    fn has_stubs_no_capabilities() {
        // A manifest with no [capabilities] block → no stubs.
        let manifest = parse_manifest(
            r#"
[plugin]
id      = "test"
name    = "test"
version = "0.1.0"
"#,
        );
        assert!(!has_stubs(&manifest));
    }

    #[test]
    fn has_stubs_catalog_with_id_sources_no_stubs() {
        // A manifest with [capabilities.catalog] and id_sources but no stub verbs.
        let manifest = parse_manifest(
            r#"
[plugin]
id      = "test"
name    = "test"
version = "0.1.0"

[capabilities.catalog]
id_sources = ["musicbrainz", "spotify"]
"#,
        );
        assert!(!has_stubs(&manifest));
    }

    #[test]
    fn has_stubs_returns_false_even_when_toml_has_stub_keys() {
        // Until CatalogCapability gains per-verb sub-tables, the SDK drops
        // unknown keys on deserialization and has_stubs can't detect them.
        // This test documents that known limitation explicitly.
        let manifest = parse_manifest(
            r#"
[plugin]
id      = "test"
name    = "test"
version = "0.1.0"

[capabilities.catalog]
id_sources = ["musicbrainz"]
"#,
        );
        // Currently always false — see has_stubs doc-comment.
        assert!(!has_stubs(&manifest));
    }

    /// Verify that cargo's dash→underscore normalization is applied when
    /// deriving the `.wasm` artifact name from the crate's `package.name`.
    #[test]
    fn wasm_artifact_path_normalises_dashes_to_underscores() {
        let tmp = std::env::temp_dir().join("stui_build_test_dash_norm");
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(
            tmp.join("Cargo.toml"),
            "[package]\nname = \"my-cool-plugin\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();

        let path = wasm_artifact_path(&tmp, "debug").expect("should produce a path");
        let file_name = path.file_name().unwrap().to_string_lossy();
        assert_eq!(
            file_name, "my_cool_plugin.wasm",
            "dashes in crate name must be replaced with underscores in the artifact file name"
        );
        std::fs::remove_dir_all(&tmp).ok();
    }
}
