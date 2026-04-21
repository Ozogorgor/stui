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

    // ── Step 3: lint ─────────────────────────────────────────────────────────
    //
    // Run the full lint suite against the already-parsed manifest.  Lint
    // failures are build failures — the `?` propagates immediately.
    crate::cmd::lint::run_manifest(&manifest)
        .context("lint failed; fix the reported issues and retry")?;

    // ── Step 4: stub check (--release guard) ─────────────────────────────────
    //
    // Tier-3 registry gate: external plugins must not ship with declared
    // stubs. Bundled plugins (built without `--release`) are allowed to
    // ship stubs; the lint step above emits a warning for them but lets
    // them through.

    if release {
        let stubs = declared_stubs(&manifest);
        if !stubs.is_empty() {
            anyhow::bail!(
                "--release build rejected: plugin has stubbed verbs {:?}. Remove `stub = true` from [capabilities.catalog] or drop --release.",
                stubs,
            );
        }
    }

    // ── Step 5: report artifact path ─────────────────────────────────────────

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

/// Return the names of every verb the manifest declares as a stub via
/// `{ stub = true, reason = "..." }`. Empty vec = no stubs.
///
/// Checks each optional verb on `[capabilities.catalog]` (`lookup`,
/// `enrich`, `artwork`, `credits`, `related`) using the `is_stub()` helpers
/// on the corresponding config types. `search` cannot be stubbed — it's
/// a plain `Option<bool>` and is required for any catalog plugin.
fn declared_stubs(manifest: &stui_plugin_sdk::PluginManifest) -> Vec<&'static str> {
    use stui_plugin_sdk::CatalogCapability;

    let mut stubs = Vec::new();
    if let CatalogCapability::Typed { lookup, enrich, artwork, credits, related, .. } =
        &manifest.capabilities.catalog
    {
        if let Some(l) = lookup  { if l.is_stub() { stubs.push("lookup");  } }
        if let Some(e) = enrich  { if e.is_stub() { stubs.push("enrich");  } }
        if let Some(a) = artwork { if a.is_stub() { stubs.push("artwork"); } }
        if let Some(c) = credits { if c.is_stub() { stubs.push("credits"); } }
        if let Some(r) = related { if r.is_stub() { stubs.push("related"); } }
    }
    stubs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_manifest(toml_str: &str) -> stui_plugin_sdk::PluginManifest {
        toml::from_str(toml_str).expect("test manifest should parse")
    }

    #[test]
    fn declared_stubs_empty_when_no_capabilities() {
        let manifest = parse_manifest(
            r#"
[plugin]
id      = "test"
name    = "test"
version = "0.1.0"
"#,
        );
        assert!(declared_stubs(&manifest).is_empty());
    }

    #[test]
    fn declared_stubs_empty_when_verbs_all_live() {
        let manifest = parse_manifest(
            r#"
[plugin]
id      = "test"
name    = "test"
version = "0.1.0"

[capabilities.catalog]
kinds   = ["movie"]
search  = true
lookup  = { id_sources = ["imdb"] }
"#,
        );
        assert!(declared_stubs(&manifest).is_empty());
    }

    #[test]
    fn declared_stubs_catches_stubbed_related() {
        let manifest = parse_manifest(
            r#"
[plugin]
id      = "test"
name    = "test"
version = "0.1.0"

[capabilities.catalog]
kinds   = ["album"]
search  = true
related = { stub = true, reason = "recommendations endpoint pending" }
"#,
        );
        assert_eq!(declared_stubs(&manifest), vec!["related"]);
    }

    #[test]
    fn declared_stubs_catches_multiple_verbs() {
        let manifest = parse_manifest(
            r#"
[plugin]
id      = "test"
name    = "test"
version = "0.1.0"

[capabilities.catalog]
kinds   = ["album"]
search  = true
lookup  = { stub = true, reason = "lookup pending" }
enrich  = { stub = true, reason = "enrich pending" }
"#,
        );
        let s = declared_stubs(&manifest);
        assert!(s.contains(&"lookup"));
        assert!(s.contains(&"enrich"));
        assert_eq!(s.len(), 2);
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
