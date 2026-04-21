use anyhow::{Context, Result};

/// Run lint against the manifest in `manifest` (already parsed).
///
/// Separated from `run()` so that `build.rs` can pass the already-parsed
/// manifest and avoid a redundant disk read.
pub fn run_manifest(manifest: &stui_plugin_sdk::PluginManifest) -> Result<()> {
    // ── Check 1: validate_manifest (legacy field rejection + id-source canonicality) ──
    stui_plugin_sdk::capabilities::validate_manifest(manifest)
        .map_err(|e| anyhow::anyhow!("manifest validation failed: {e}"))?;

    // ── Check 2: required config fields have label + hint ─────────────────────
    //
    // TODO(Task 1.7 follow-on): The SDK's `PluginManifest` does not yet carry a
    // `[[config]]` array (there is no `PluginConfigField` type in the SDK).
    // When the schema is extended to include config-field declarations, iterate
    // `manifest.config` here and warn on required fields that are missing
    // `label` or `hint`.  Tracked as part of the CatalogCapability per-verb
    // sub-table expansion.

    // ── Check 3: stub-verb warnings ───────────────────────────────────────────
    //
    // TODO(Task 1.7 follow-on / stub-verb manifest reconciliation): The SDK
    // `CatalogCapability` does not yet encode a `stub = true` flag per verb.
    // When it does, iterate over declared verbs and emit a warning for each
    // that is marked as a stub.  Pending SDK manifest reconciliation.

    let warnings: u32 = 0;

    if warnings > 0 {
        println!("Lint completed with {warnings} warning(s).");
    } else {
        println!("Lint OK.");
    }
    Ok(())
}

/// Standalone entry point for `stui plugin lint`.
///
/// Reads and parses `plugin.toml` from the current directory, then delegates
/// to [`run_manifest`].
pub fn run() -> Result<()> {
    let cwd = std::env::current_dir()?;
    let manifest_path = cwd.join("plugin.toml");
    if !manifest_path.exists() {
        anyhow::bail!("no plugin.toml in current directory");
    }

    let manifest_text =
        std::fs::read_to_string(&manifest_path).context("read plugin.toml")?;
    let manifest: stui_plugin_sdk::PluginManifest = toml::from_str(&manifest_text)
        .context("plugin.toml is not valid TOML against PluginManifest schema")?;

    run_manifest(&manifest)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(toml_str: &str) -> stui_plugin_sdk::PluginManifest {
        toml::from_str(toml_str).expect("test manifest should parse")
    }

    #[test]
    fn lint_passes_minimal_valid_manifest() {
        // Minimal valid canonical manifest: typed catalog with `search = true`.
        // Legacy bool form `[capabilities] catalog = true` also passes but
        // carries no scope information — we standardize on the typed form.
        let manifest = parse(
            r#"
[plugin]
id      = "my-plugin"
name    = "My Plugin"
version = "0.1.0"

[capabilities.catalog]
kinds  = ["movie"]
search = true
"#,
        );
        assert!(run_manifest(&manifest).is_ok());
    }

    #[test]
    fn lint_passes_manifest_with_canonical_id_sources() {
        // Canonical id-sources now live in `[capabilities.catalog.lookup]`.
        let manifest = parse(
            r#"
[plugin]
id      = "tmdb-catalog"
name    = "TMDB Catalog"
version = "0.1.0"

[capabilities.catalog]
kinds  = ["movie"]
search = true

[capabilities.catalog.lookup]
id_sources = ["tmdb", "imdb"]
"#,
        );
        assert!(run_manifest(&manifest).is_ok());
    }

    #[test]
    fn lint_fails_on_unknown_id_source() {
        let manifest = parse(
            r#"
[plugin]
id      = "bad-plugin"
name    = "Bad Plugin"
version = "0.1.0"

[capabilities.catalog]
kinds  = ["movie"]
search = true

[capabilities.catalog.lookup]
id_sources = ["definitely_not_a_real_source_xyz"]
"#,
        );
        let err = run_manifest(&manifest).unwrap_err();
        assert!(
            err.to_string().contains("manifest validation failed"),
            "error should mention manifest validation: {err}"
        );
        assert!(
            err.to_string().contains("definitely_not_a_real_source_xyz"),
            "error should name the bad id-source: {err}"
        );
    }

    #[test]
    fn lint_fails_on_legacy_network_bool() {
        // `network = true` is the legacy form; validate_manifest rejects it.
        let manifest: stui_plugin_sdk::PluginManifest = toml::from_str(
            r#"
[plugin]
id      = "legacy-plugin"
name    = "Legacy Plugin"
version = "0.1.0"

[permissions]
network = true

[capabilities.catalog]
kinds  = ["movie"]
search = true
"#,
        )
        .expect("should parse as toml");
        let err = run_manifest(&manifest).unwrap_err();
        assert!(
            err.to_string().contains("manifest validation failed"),
            "error should mention manifest validation: {err}"
        );
    }

    #[test]
    fn lint_passes_manifest_with_host_allowlist() {
        // `network = ["host"]` is the canonical form; should pass.
        let manifest = parse(
            r#"
[plugin]
id      = "net-plugin"
name    = "Net Plugin"
version = "0.1.0"

[permissions]
network = ["api.example.com"]

[capabilities.catalog]
kinds  = ["movie"]
search = true
"#,
        );
        assert!(run_manifest(&manifest).is_ok());
    }

    #[test]
    fn lint_fails_when_catalog_search_missing() {
        // New canonical-schema enforcement: the CLI validator now catches
        // `[capabilities.catalog]` without `search = true`. This is the
        // proof-of-consolidation test: with the old stub-shape SDK the
        // omission passed silently; with the authoritative SDK it fails.
        let manifest = parse(
            r#"
[plugin]
id      = "no-search"
name    = "No Search"
version = "0.1.0"

[capabilities.catalog]
kinds = ["movie"]
"#,
        );
        let err = run_manifest(&manifest).unwrap_err();
        assert!(
            err.to_string().contains("required verb not declared"),
            "error should mention required verb: {err}"
        );
        assert!(
            err.to_string().contains("search"),
            "error should name the search verb: {err}"
        );
    }
}
