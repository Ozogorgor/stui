//! Integration tests for plugin hot-reload behaviour with invalid manifests.
//!
//! These tests verify that:
//!   - Loading a manifest that is missing required fields returns a clear `Err`
//!     and does NOT panic.
//!   - Loading an entirely empty `plugin.toml` also returns a clear `Err`.
//!   - In both cases the runtime function returns, so the caller can continue
//!     operating normally.

use stui_runtime::plugin::load_manifest;

/// A `plugin.toml` that omits the required `[plugin]` table entirely.
/// `toml::from_str` must fail rather than panic, and the error should be
/// descriptive enough for the user to understand what is wrong.
#[test]
fn invalid_manifest_missing_required_fields() {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let manifest_path = dir.path().join("plugin.toml");

    // Write a TOML file that deliberately omits the required `[plugin]` table.
    std::fs::write(
        &manifest_path,
        r#"
[meta]
author = "test"
"#,
    )
    .expect("failed to write plugin.toml");

    let result = load_manifest(dir.path());

    assert!(
        result.is_err(),
        "expected Err for missing required fields, got Ok"
    );

    let err_msg = format!("{:#}", result.unwrap_err());
    // The error must contain at least one of these tokens to be considered
    // actionable for the developer reading the log.
    let is_descriptive = err_msg.contains("plugin")
        || err_msg.contains("missing")
        || err_msg.contains("field")
        || err_msg.contains("parsing");

    assert!(
        is_descriptive,
        "error message is not descriptive enough: {err_msg}"
    );
}

/// An entirely empty `plugin.toml` must also result in an `Err`, not a panic.
/// This covers the edge case where a file is created but not yet written
/// during a hot-reload cycle.
#[test]
fn empty_toml_file_returns_error() {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let manifest_path = dir.path().join("plugin.toml");

    std::fs::write(&manifest_path, "").expect("failed to write empty plugin.toml");

    let result = load_manifest(dir.path());

    assert!(
        result.is_err(),
        "expected Err for empty plugin.toml, got Ok"
    );
}
