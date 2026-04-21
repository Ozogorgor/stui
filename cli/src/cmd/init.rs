use anyhow::{Context, Result};
use std::path::PathBuf;

const CARGO_TOML: &str = include_str!("../template/Cargo.toml.template");
const PLUGIN_TOML: &str = include_str!("../template/plugin.toml.template");
const LIB_RS: &str = include_str!("../template/src/lib.rs.template");
const BASIC_TEST: &str = include_str!("../template/tests/basic.rs.template");
const FIXTURE: &str = include_str!("../template/tests/fixtures/example.json");
const README: &str = include_str!("../template/README.md.template");

pub fn run(name: String, dir: Option<PathBuf>) -> Result<()> {
    // Validate plugin name: must be a valid Rust identifier for the PLUGIN_TYPE
    // (after CamelCase conversion) and a valid cargo package name (kebab-case OK
    // with the `-provider` suffix).
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        anyhow::bail!("plugin name must be non-empty ASCII alphanumeric (dash/underscore allowed): {name}");
    }

    let dir = dir.unwrap_or_else(|| PathBuf::from(format!("{}-provider", name)));
    if dir.exists() {
        anyhow::bail!("target directory already exists: {}", dir.display());
    }

    // Compute substitutions
    let plugin_type = to_camel_case(&name);
    let plugin_type = format!("{}Plugin", plugin_type);
    let plugin_name_underscored = name.replace('-', "_");

    let subst = |s: &str| {
        s.replace("{{PLUGIN_NAME}}", &name)
         .replace("{{PLUGIN_TYPE}}", &plugin_type)
         .replace("{{PLUGIN_NAME_UNDERSCORED}}", &plugin_name_underscored)
    };

    std::fs::create_dir_all(&dir).with_context(|| format!("create dir {}", dir.display()))?;
    std::fs::create_dir_all(dir.join("src"))?;
    std::fs::create_dir_all(dir.join("tests").join("fixtures"))?;

    std::fs::write(dir.join("Cargo.toml"), subst(CARGO_TOML))?;
    std::fs::write(dir.join("plugin.toml"), subst(PLUGIN_TOML))?;
    std::fs::write(dir.join("src/lib.rs"), subst(LIB_RS))?;
    std::fs::write(dir.join("tests/basic.rs"), subst(BASIC_TEST))?;
    std::fs::write(dir.join("tests/fixtures/example.json"), FIXTURE)?;
    std::fs::write(dir.join("README.md"), subst(README))?;

    println!("Scaffolded plugin '{}' at {}", name, dir.display());
    println!("Next: cd {} && stui plugin build && stui plugin lint", dir.display());
    Ok(())
}

/// Convert `music-brainz` / `music_brainz` / `musicbrainz` → `MusicBrainz` / `MusicBrainz` / `Musicbrainz`.
/// Each `-` or `_` separator yields a fresh uppercase letter.
fn to_camel_case(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut uppercase_next = true;
    for ch in name.chars() {
        if ch == '-' || ch == '_' {
            uppercase_next = true;
            continue;
        }
        if uppercase_next {
            out.extend(ch.to_uppercase());
            uppercase_next = false;
        } else {
            out.push(ch);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camel_case_conversions() {
        assert_eq!(to_camel_case("musicbrainz"), "Musicbrainz");
        assert_eq!(to_camel_case("music-brainz"), "MusicBrainz");
        assert_eq!(to_camel_case("music_brainz"), "MusicBrainz");
        assert_eq!(to_camel_case("tmdb"), "Tmdb");
        assert_eq!(to_camel_case("last-fm"), "LastFm");
    }
}
