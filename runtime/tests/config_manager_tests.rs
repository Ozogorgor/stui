//! Integration tests for `ConfigManager` — runtime live config updates.
//!
//! Tests the full round-trip: apply a `SetConfig` key → verify snapshot
//! reflects the change → verify `ConfigChanged` event was emitted.

use std::sync::Arc;
use stui_runtime::config::{ConfigManager, RuntimeConfig};
use stui_runtime::events::{EventBus, RuntimeEvent};

fn make_manager() -> (ConfigManager, Arc<EventBus>) {
    let bus = Arc::new(EventBus::new());
    let mgr = ConfigManager::new(RuntimeConfig::default(), bus.clone());
    (mgr, bus)
}

// ── Player config ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn set_player_volume() {
    let (mgr, _) = make_manager();
    mgr.set_number("player.default_volume", 80.0).await.unwrap();
    let snap = mgr.snapshot().await;
    assert!((snap.playback.default_volume - 80.0).abs() < 1e-6);
}

#[tokio::test]
async fn set_player_hwdec() {
    let (mgr, _) = make_manager();
    mgr.set_str("player.hwdec", "vaapi").await.unwrap();
    let snap = mgr.snapshot().await;
    assert_eq!(snap.playback.hwdec, "vaapi");
}

#[tokio::test]
async fn set_player_cache_secs() {
    let (mgr, _) = make_manager();
    mgr.set_number("player.cache_secs", 30_f64).await.unwrap();
    let snap = mgr.snapshot().await;
    assert_eq!(snap.playback.cache_secs, 30);
}

#[tokio::test]
async fn set_player_keep_open() {
    let (mgr, _) = make_manager();
    mgr.set_bool("player.keep_open", true).await.unwrap();
    let snap = mgr.snapshot().await;
    assert!(snap.playback.keep_open);
}

// ── Streaming config ──────────────────────────────────────────────────────────

#[tokio::test]
async fn set_streaming_prefer_torrent() {
    let (mgr, _) = make_manager();
    mgr.set_bool("streaming.prefer_torrent", true)
        .await
        .unwrap();
    let snap = mgr.snapshot().await;
    assert!(snap.streaming.prefer_torrent);
}

#[tokio::test]
async fn set_streaming_auto_fallback_off() {
    let (mgr, _) = make_manager();
    mgr.set_bool("streaming.auto_fallback", false)
        .await
        .unwrap();
    let snap = mgr.snapshot().await;
    assert!(!snap.streaming.auto_fallback);
}

#[tokio::test]
async fn set_streaming_max_candidates() {
    let (mgr, _) = make_manager();
    mgr.set_number("streaming.max_candidates", 5.0)
        .await
        .unwrap();
    let snap = mgr.snapshot().await;
    assert_eq!(snap.streaming.max_candidates, 5);
}

// ── Subtitles config ──────────────────────────────────────────────────────────

#[tokio::test]
async fn set_subtitles_language() {
    let (mgr, _) = make_manager();
    mgr.set_str("subtitles.preferred_language", "fra")
        .await
        .unwrap();
    let snap = mgr.snapshot().await;
    assert_eq!(snap.subtitles.preferred_language, "fra");
}

#[tokio::test]
async fn set_subtitles_default_delay() {
    let (mgr, _) = make_manager();
    mgr.set_number("subtitles.default_delay", 1.5)
        .await
        .unwrap();
    let snap = mgr.snapshot().await;
    assert!((snap.subtitles.default_delay - 1.5).abs() < 1e-6);
}

// ── Provider config ───────────────────────────────────────────────────────────

#[tokio::test]
async fn disable_tmdb() {
    let (mgr, _) = make_manager();
    mgr.set_bool("providers.enable_tmdb", false).await.unwrap();
    let snap = mgr.snapshot().await;
    assert!(!snap.providers.enable_tmdb);
}

#[tokio::test]
async fn enable_prowlarr() {
    let (mgr, _) = make_manager();
    mgr.set_bool("providers.enable_prowlarr", true)
        .await
        .unwrap();
    let snap = mgr.snapshot().await;
    assert!(snap.providers.enable_prowlarr);
}

// ── App config ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn set_theme_mode() {
    let (mgr, _) = make_manager();
    mgr.set_str("app.theme_mode", "light").await.unwrap();
    let snap = mgr.snapshot().await;
    assert_eq!(snap.theme_mode, "light");
}

// ── EventBus broadcast ────────────────────────────────────────────────────────

#[tokio::test]
async fn set_emits_config_changed_event() {
    let (mgr, bus) = make_manager();
    let mut rx = bus.subscribe();

    mgr.set_number("player.default_volume", 42.0).await.unwrap();

    // Should have received a ConfigChanged event
    let event = tokio::time::timeout(std::time::Duration::from_millis(100), async {
        rx.recv().await
    })
    .await
    .expect("timeout waiting for event")
    .expect("channel closed");

    match event {
        RuntimeEvent::ConfigChanged { key, .. } => {
            assert_eq!(key, "player.default_volume");
        }
        other => panic!("expected ConfigChanged, got {:?}", other.name()),
    }
}

// ── Error cases ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn unknown_key_returns_error() {
    let (mgr, _) = make_manager();
    let result = mgr.set_str("player.nonexistent_field", "value").await;
    assert!(result.is_err(), "unknown key should return Err");
    let err = result.unwrap_err();
    assert!(err.to_string().contains("unknown config key"));
}

#[tokio::test]
async fn wrong_type_returns_error() {
    let (mgr, _) = make_manager();
    // volume expects a number, not a string
    let result = mgr
        .set(
            "player.default_volume",
            serde_json::Value::String("loud".into()),
        )
        .await;
    assert!(result.is_err(), "wrong type should return Err");
}

#[tokio::test]
async fn multiple_updates_accumulate() {
    let (mgr, _) = make_manager();
    mgr.set_number("player.default_volume", 50.0).await.unwrap();
    mgr.set_str("player.hwdec", "nvdec").await.unwrap();
    mgr.set_bool("streaming.prefer_torrent", true)
        .await
        .unwrap();

    let snap = mgr.snapshot().await;
    assert!((snap.playback.default_volume - 50.0).abs() < 1e-6);
    assert_eq!(snap.playback.hwdec, "nvdec");
    assert!(snap.streaming.prefer_torrent);
}
