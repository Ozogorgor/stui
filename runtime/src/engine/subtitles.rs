//! Subtitle fan-out pipeline.
//!
//! Called from `PlayerBridge::play` when `config.subtitles.auto_download`
//! is on. Fans `stui_search` across every enabled Subtitles-capability
//! plugin, filters to `preferred_language`, returns the top 5 candidates.
//! The caller is responsible for calling `stui_resolve` on the chosen
//! candidate and downloading the subtitle file.

use std::time::Duration;

use tokio::task::JoinSet;
use tracing::warn;

use stui_plugin_sdk::{EntryKind, SearchScope};

use crate::abi::types::PluginEntry as AbiPluginEntry;
use crate::abi::SearchRequest as AbiSearchRequest;
use crate::engine::Engine;
use crate::plugin::PluginCapability;

/// One subtitle candidate from a single plugin.
#[derive(Debug, Clone)]
pub struct SubtitleCandidate {
    pub plugin_id: String,
    pub plugin_name: String,
    pub entry: AbiPluginEntry,
    /// Best-effort language extraction, BCP-47 or ISO 639-1/2/3.
    /// None if the plugin didn't surface language info in the entry.
    pub language: Option<String>,
}

/// Fan subtitle search across every enabled Subtitles-capability plugin.
///
/// Per-plugin timeout: 10s. Language match: case-insensitive, normalized
/// 2-char / 3-char / full-name forms.
///
/// Returns top 5 candidates sorted by (language exact match, imdb_id
/// match, description richness). Empty vec on "no matches" — not an error.
pub async fn fetch_subtitles(
    engine: &Engine,
    title: &str,
    imdb_id: Option<&str>,
    kind: EntryKind,
    language: &str,
) -> Vec<SubtitleCandidate> {
    // Collect enabled Subtitles-capability plugins under a short read-lock,
    // then drop it so per-plugin calls don't hold the registry.
    let providers: Vec<(String, String)> = {
        let reg = engine.registry_read().await;
        reg.find_by_capability(PluginCapability::Subtitles)
            .into_iter()
            .map(|p| (p.id.clone(), p.manifest.plugin.name.clone()))
            .collect()
    };

    if providers.is_empty() {
        return vec![];
    }

    let scope = kind_to_scope(kind);
    let query_owned = title.to_string();
    let locale_owned = language.to_string();

    // Fan out. Each task owns an Engine clone (cheap — Arc internal state).
    let mut set: JoinSet<(String, String, Option<Vec<AbiPluginEntry>>)> = JoinSet::new();
    for (plugin_id, plugin_name) in providers {
        let req = AbiSearchRequest {
            query: query_owned.clone(),
            scope,
            page: 0,
            limit: 20,
            per_scope_limit: Some(20),
            locale: Some(locale_owned.clone()),
        };
        let engine_c = engine.clone();
        set.spawn(async move {
            let entries = match tokio::time::timeout(
                Duration::from_secs(10),
                call_plugin_search(&engine_c, &plugin_id, req),
            )
            .await
            {
                Ok(Ok(items)) => Some(items),
                Ok(Err(e)) => {
                    warn!(plugin = %plugin_id, err = %e,
                          "subtitle plugin search failed");
                    None
                }
                Err(_) => {
                    warn!(plugin = %plugin_id,
                          "subtitle plugin search timed out (10s)");
                    None
                }
            };
            (plugin_id, plugin_name, entries)
        });
    }

    let normalized_lang = normalize_language(language);
    let target_imdb = imdb_id.map(str::to_string);

    let mut candidates: Vec<SubtitleCandidate> = Vec::new();
    while let Some(Ok((plugin_id, plugin_name, maybe_entries))) = set.join_next().await {
        let Some(entries) = maybe_entries else {
            continue;
        };
        for entry in entries {
            let lang = extract_language(&entry);
            candidates.push(SubtitleCandidate {
                plugin_id: plugin_id.clone(),
                plugin_name: plugin_name.clone(),
                entry,
                language: lang,
            });
        }
    }

    // Filter: keep only candidates whose language matches. If nothing
    // matches, keep everything (unknown-language candidates are better than
    // no candidates — caller decides whether to use them).
    let (matching, unknown): (Vec<_>, Vec<_>) = candidates.into_iter().partition(|c| {
        c.language
            .as_ref()
            .map(|l| normalize_language(l) == normalized_lang)
            .unwrap_or(false)
    });
    let mut candidates = if !matching.is_empty() {
        matching
    } else {
        unknown
    };

    // Sort: imdb_id match first, then description richness.
    candidates.sort_by(|a, b| {
        let a_imdb = target_imdb.as_deref() == a.entry.imdb_id.as_deref();
        let b_imdb = target_imdb.as_deref() == b.entry.imdb_id.as_deref();
        b_imdb.cmp(&a_imdb).then_with(|| {
            let a_desc = a.entry.description.as_deref().map(str::len).unwrap_or(0);
            let b_desc = b.entry.description.as_deref().map(str::len).unwrap_or(0);
            b_desc.cmp(&a_desc)
        })
    });

    candidates.truncate(5);
    candidates
}

/// Look up a plugin's WASM supervisor and call `stui_search`, returning
/// the raw `abi::PluginEntry` list. Error mapping is string-flattened
/// here — callers don't need the structured error taxonomy.
async fn call_plugin_search(
    engine: &Engine,
    plugin_id: &str,
    req: AbiSearchRequest,
) -> Result<Vec<AbiPluginEntry>, String> {
    let sup = {
        let reg = engine.registry_read().await;
        match reg.resolve_id(plugin_id) {
            Some(canonical) => reg.wasm_supervisor_for(canonical),
            None => return Err(format!("plugin '{plugin_id}' not found")),
        }
    };
    let sup = sup.ok_or_else(|| format!("plugin '{plugin_id}' has no WASM supervisor"))?;
    let resp = sup.search(&req).await.map_err(|e| e.to_string())?;
    Ok(resp.items)
}

/// Same shape as `call_plugin_search` but for `stui_resolve`. Public so
/// `PlayerBridge::download_subtitle` can use it.
pub async fn call_plugin_resolve(
    engine: &Engine,
    plugin_id: &str,
    entry_id: &str,
) -> Result<crate::abi::types::ResolveResponse, String> {
    let sup = {
        let reg = engine.registry_read().await;
        match reg.resolve_id(plugin_id) {
            Some(canonical) => reg.wasm_supervisor_for(canonical),
            None => return Err(format!("plugin '{plugin_id}' not found")),
        }
    };
    let sup = sup.ok_or_else(|| format!("plugin '{plugin_id}' has no WASM supervisor"))?;
    let req = crate::abi::types::ResolveRequest {
        entry_id: entry_id.to_string(),
    };
    sup.resolve(&req).await.map_err(|e| e.to_string())
}

fn kind_to_scope(kind: EntryKind) -> SearchScope {
    match kind {
        EntryKind::Movie => SearchScope::Movie,
        EntryKind::Series | EntryKind::Episode => SearchScope::Series,
        // Music kinds are nonsensical for subtitles — plugins will
        // UNSUPPORTED_SCOPE and return empty. Callers typically won't
        // reach this branch since kind is derived from the play request's
        // media tab.
        _ => SearchScope::Movie,
    }
}

/// Lowercase, trim, normalize ISO 639-2/3 and full-name forms to 2-char.
fn normalize_language(lang: &str) -> String {
    let l = lang.trim().to_lowercase();
    match l.as_str() {
        "eng" | "english" => "en".into(),
        "spa" | "es" | "spanish" => "es".into(),
        "fre" | "fra" | "french" => "fr".into(),
        "ger" | "deu" | "german" => "de".into(),
        "ita" | "italian" => "it".into(),
        "por" | "portuguese" => "pt".into(),
        "rus" | "russian" => "ru".into(),
        "jpn" | "japanese" => "ja".into(),
        "kor" | "korean" => "ko".into(),
        "chi" | "zho" | "chinese" => "zh".into(),
        "ara" | "arabic" => "ar".into(),
        _ if l.len() > 3 => l,
        _ => l,
    }
}

fn extract_language(entry: &AbiPluginEntry) -> Option<String> {
    if let Some(lang) = &entry.original_language {
        if !lang.is_empty() {
            return Some(lang.clone());
        }
    }
    let haystack = format!(
        "{} {}",
        entry.title.to_lowercase(),
        entry.description.as_deref().unwrap_or("").to_lowercase(),
    );
    for token in [
        "english",
        "spanish",
        "french",
        "german",
        "italian",
        "portuguese",
        "russian",
        "japanese",
        "korean",
        "chinese",
        "arabic",
    ] {
        if haystack.contains(token) {
            return Some(normalize_language(token));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::abi::types::PluginEntry;

    #[test]
    fn language_normalization() {
        assert_eq!(normalize_language("en"), "en");
        assert_eq!(normalize_language("eng"), "en");
        assert_eq!(normalize_language("English"), "en");
        assert_eq!(normalize_language("ES"), "es");
    }

    #[test]
    fn extract_from_original_language() {
        let entry = PluginEntry {
            id: "x".into(),
            title: "whatever".into(),
            original_language: Some("fr".into()),
            ..Default::default()
        };
        assert_eq!(extract_language(&entry), Some("fr".into()));
    }

    #[test]
    fn extract_from_description_keyword() {
        let entry = PluginEntry {
            id: "x".into(),
            title: "The Movie".into(),
            description: Some("English SDH".into()),
            ..Default::default()
        };
        assert_eq!(extract_language(&entry), Some("en".into()));
    }
}
