//! Roon server integration for stui
//!
//! This module provides:
//! - mDNS discovery for finding Roon servers on the local network
//! - WebSocket connection for real-time updates
//! - Token management for authentication
//!
//! Roon uses a unique architecture where it acts as a controller for audio endpoints.
//! The API is accessed via WebSocket on port 9330.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc, RwLock};
use tokio_tungstenite::{connect_async, tungstenite::{Message, Utf8Bytes}};
use tracing::{debug, error, info, warn};

use futures_util::{SinkExt, StreamExt};

// ── Constants ─────────────────────────────────────────────────────────────────

const ROON_SERVICE_TYPE: &str = "_roon._tcp.local.";
const ROON_SERVICE_PORT: u16 = 9330;
const ROON_APP_ID: &str = "stui_roon";
const ROON_APP_NAME: &str = "stui";

// ── Data Structures ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoonServer {
    pub host: String,
    pub port: u16,
    pub core_id: String,
    pub display_name: String,
    pub token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoonConfig {
    pub servers: Vec<RoonServer>,
    pub selected_core: Option<String>,
}

impl Default for RoonConfig {
    fn default() -> Self {
        Self {
            servers: Vec::new(),
            selected_core: None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum RoonEvent {
    ZoneChanged(String),
    PlaybackStateChanged(String),
    NowPlayingChanged(String),
    Connected,
    Disconnected,
    Error(String),
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "method")]
enum RoonRequest {
    #[serde(rename = "subscribe_zones")]
    SubscribeZones { },
    
    #[serde(rename = "subscribe_outputs")]
    SubscribeOutputs { },
    
    #[serde(rename = "player_volume")]
    PlayerVolume {
        zone_or_output_id: String,
        volume: u8,
    },
    
    #[serde(rename = "player_play")]
    PlayerPlay { zone_or_output_id: String },
    
    #[serde(rename = "player_pause")]
    PlayerPause { zone_or_output_id: String },
    
    #[serde(rename = "player_toggle")]
    PlayerToggle { zone_or_output_id: String },
    
    #[serde(rename = "player_previous")]
    PlayerPrevious { zone_or_output_id: String },
    
    #[serde(rename = "player_next")]
    PlayerNext { zone_or_output_id: String },
    
    #[serde(rename = "player_seek")]
    PlayerSeek {
        zone_or_output_id: String,
        seek_position: u64,
    },
    
    #[serde(rename = "browse")]
    Browse {
        browse_key: String,
        offset: u32,
        limit: u32,
    },
    
    #[serde(rename = "deep_search")]
    DeepSearch {
        search_type: String,
        search_query: String,
        offset: u32,
        limit: u32,
    },
    
    #[serde(rename = "queue_and_play")]
    QueueAndPlay {
        queue_item_key: String,
        seek_position: Option<u64>,
    },
}

// ── Roon Client ─────────────────────────────────────────────────────────────

pub struct RoonClient {
    config: Arc<RwLock<RoonConfig>>,
    event_tx: broadcast::Sender<RoonEvent>,
    ws_sender: Arc<RwLock<Option<mpsc::Sender<String>>>>,
    shutdown_tx: Arc<RwLock<Option<mpsc::Sender<String>>>>,
}

impl RoonClient {
    pub fn new() -> Self {
        let (event_tx, _) = broadcast::channel(100);
        Self {
            config: Arc::new(RwLock::new(RoonConfig::default())),
            event_tx,
            ws_sender: Arc::new(RwLock::new(None)),
            shutdown_tx: Arc::new(RwLock::new(None)),
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<RoonEvent> {
        self.event_tx.subscribe()
    }

    /// Discover Roon servers on the local network using mDNS.
    ///
    /// Polls for up to 2.5 s (5 × 500 ms quiet periods) after the last event.
    pub async fn discover(&self) -> Result<Vec<RoonServer>> {
        info!("Starting Roon mDNS discovery...");

        let daemon = mdns_sd::ServiceDaemon::new()?;
        let receiver = daemon.browse(ROON_SERVICE_TYPE)?;

        // Move the blocking mdns_sd receiver onto a dedicated thread and
        // bridge it with a tokio channel so we never block the async executor.
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Option<mdns_sd::ServiceEvent>>(32);

        std::thread::spawn(move || {
            loop {
                match receiver.recv() {
                    Ok(event) => {
                        if tx.blocking_send(Some(event)).is_err() {
                            break;
                        }
                    }
                    Err(_) => {
                        let _ = tx.blocking_send(None);
                        break;
                    }
                }
            }
        });

        let mut servers = Vec::new();
        let mut timeout_count = 0u32;

        loop {
            match tokio::time::timeout(Duration::from_millis(500), rx.recv()).await {
                Ok(Some(Some(event))) => {
                    if let mdns_sd::ServiceEvent::ServiceResolved(info) = event {
                        let host = info
                            .get_addresses()
                            .iter()
                            .find_map(|addr| match addr {
                                std::net::IpAddr::V4(v4) => Some(v4.to_string()),
                                _ => None,
                            })
                            .unwrap_or_else(|| "127.0.0.1".to_string());

                        let port = info.get_port();
                        let core_id = info
                            .get_property_val_str("coreId")
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| "unknown".to_string());
                        let display_name = info
                            .get_property_val_str("name")
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| "Unknown Roon".to_string());

                        info!(host, port, core_id, name = %display_name, "Found Roon server");

                        servers.push(RoonServer {
                            host,
                            port,
                            core_id,
                            display_name,
                            token: None,
                        });
                    }
                    timeout_count = 0;
                }
                // Thread finished or channel closed
                Ok(Some(None)) | Ok(None) => break,
                // 500 ms with no event — count quiet periods
                Err(_) => {
                    timeout_count += 1;
                    if timeout_count >= 5 {
                        break;
                    }
                }
            }
        }

        let mut config = self.config.write().await;
        config.servers = servers.clone();

        info!(count = servers.len(), "Roon discovery complete");
        Ok(servers)
    }

    /// Connect to a Roon server and authenticate
    pub async fn connect(&self, server: &RoonServer) -> Result<()> {
        info!(host = %server.host, "Connecting to Roon server...");
        
        let url = format!("ws://{}:{}", server.host, server.port);
        
        let (ws_stream, _) = connect_async(&url).await?;
        let (mut write, mut read) = ws_stream.split();
        
        let (tx, mut rx) = mpsc::channel::<String>(100);
        let (shutdown_tx, mut shutdown_rx) = mpsc::channel::<String>(100);
        
        {
            let mut sender = self.ws_sender.write().await;
            *sender = Some(tx.clone());
        }
        {
            let mut shutdown = self.shutdown_tx.write().await;
            *shutdown = Some(shutdown_tx);
        }
        
        let handshake = serde_json::json!({
            "method": "register",
            "protocol": [
                {
                    "name": " antiquated",
                    "version": "1.0"
                }
            ],
            "softname": format!("{} {}", ROON_APP_NAME, env!("CARGO_PKG_VERSION")),
            "device_id": "stui",
            "client_id": ROON_APP_ID,
            "core_id": server.core_id,
        });
        
        write.send(Message::Text(Utf8Bytes::from(handshake.to_string()))).await?;
        
        let write_tx = tx.clone();
        tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                if write.send(Message::Text(Utf8Bytes::from(msg))).await.is_err() {
                    break;
                }
            }
        });
        
        let event_tx = self.event_tx.clone();
        tokio::spawn(async move {
            while let Some(msg) = read.next().await {
                match msg {
                    Ok(Message::Text(text)) => {
                        if let Ok(response) = serde_json::from_str::<serde_json::Value>(&text) {
                            Self::handle_message(response, &event_tx);
                        }
                    }
                    Ok(Message::Close(_)) => {
                        let _ = event_tx.send(RoonEvent::Disconnected);
                        break;
                    }
                    Err(e) => {
                        error!(error = %e, "WebSocket error");
                        let _ = event_tx.send(RoonEvent::Error(e.to_string()));
                        break;
                    }
                    _ => {}
                }
            }
        });
        
        let _ = self.event_tx.send(RoonEvent::Connected);
        info!(host = %server.host, "Connected to Roon server");
        
        Ok(())
    }
    
    fn handle_message(response: serde_json::Value, event_tx: &broadcast::Sender<RoonEvent>) {
        if let Some(zones) = response.get("zones").and_then(|z| z.as_array()) {
            for zone in zones {
                if let Some(zone_id) = zone.get("zone_id").and_then(|z| z.as_str()) {
                    let _ = event_tx.send(RoonEvent::ZoneChanged(zone_id.to_string()));
                }
            }
        }
        
        if let Some(outputs) = response.get("outputs").and_then(|o| o.as_array()) {
            for output in outputs {
                if let Some(output_id) = output.get("output_id").and_then(|o| o.as_str()) {
                    let _ = event_tx.send(RoonEvent::ZoneChanged(output_id.to_string()));
                }
            }
        }
        
        if let Some(body) = response.get("body") {
            if body.get("zones").is_some() || body.get("outputs").is_some() {
                let _ = event_tx.send(RoonEvent::PlaybackStateChanged("all".to_string()));
            }
        }
    }

    pub async fn disconnect(&self) {
        if let Some(tx) = self.shutdown_tx.write().await.take() {
            let _ = tx.send("shutdown".to_string()).await;
        }
        *self.ws_sender.write().await = None;
        let _ = self.event_tx.send(RoonEvent::Disconnected);
    }

    /// Send a request over the WebSocket connection.
    ///
    /// **Limitation**: This is currently fire-and-forget. The Roon API is
    /// asynchronous — responses arrive on the read loop as independent messages
    /// keyed by `request_id`. A proper implementation would maintain a
    /// `HashMap<u32, oneshot::Sender<Value>>` and match incoming responses by
    /// `request_id`. Until that correlation map is added, callers that need a
    /// response (e.g. `search()`, `get_zones()`) must subscribe via `subscribe()`
    /// and wait for the appropriate `RoonEvent` instead of relying on the return
    /// value here.
    pub async fn request(&self, method: RoonRequest) -> Result<serde_json::Value> {
        let sender = self.ws_sender.read().await;
        let sender = sender.as_ref()
            .ok_or_else(|| anyhow!("Not connected to Roon server"))?;

        let request_id = rand_u32();
        let msg = serde_json::json!({
            "request_id": request_id,
            "method": method,
        });

        let msg_str = msg.to_string();
        sender.send(msg_str).await
            .map_err(|e| anyhow!("Failed to send: {}", e))?;

        // Response will arrive asynchronously on the read loop. Callers that
        // need the result must listen on the broadcast channel from subscribe().
        Ok(serde_json::json!({}))
    }

    pub async fn search(&self, query: &str) -> Result<Vec<RoonSearchResult>> {
        let result = self.request(RoonRequest::DeepSearch {
            search_type: "library_tracks".to_string(),
            search_query: query.to_string(),
            offset: 0,
            limit: 20,
        }).await?;
        
        let mut items = Vec::new();
        
        if let Some(body) = result.get("body") {
            if let Some(results) = body.get("results").and_then(|r| r.as_array()) {
                for item in results {
                    if let Some(item_type) = item.get("item_type").and_then(|t| t.as_str()) {
                        let id = item.get("item_key")
                            .or_else(|| item.get("track_key"))
                            .and_then(|k| k.as_str())
                            .unwrap_or("");
                        
                        let title = item.get("title")
                            .or_else(|| item.get("name"))
                            .and_then(|t| t.as_str())
                            .unwrap_or("Unknown");
                        
                        let subtitle = item.get("subtitle")
                            .or_else(|| item.get("artist"))
                            .and_then(|s| s.as_str());
                        
                        items.push(RoonSearchResult {
                            id: format!("roon:{}:{}", item_type, id),
                            title: title.to_string(),
                            subtitle: subtitle.map(String::from),
                            item_type: item_type.to_string(),
                        });
                    }
                }
            }
        }
        
        Ok(items)
    }

    pub async fn play(&self, item_key: &str) -> Result<()> {
        self.request(RoonRequest::QueueAndPlay {
            queue_item_key: item_key.to_string(),
            seek_position: None,
        }).await?;
        Ok(())
    }

    pub async fn get_zones(&self) -> Result<Vec<serde_json::Value>> {
        let result = self.request(RoonRequest::SubscribeZones {}).await?;
        
        Ok(result.get("zones")
            .and_then(|z| z.as_array())
            .map(|arr| arr.iter().cloned().collect())
            .unwrap_or_default())
    }

    pub async fn play_pause(&self, zone_id: &str) -> Result<()> {
        self.request(RoonRequest::PlayerToggle {
            zone_or_output_id: zone_id.to_string(),
        }).await?;
        Ok(())
    }

    pub async fn next(&self, zone_id: &str) -> Result<()> {
        self.request(RoonRequest::PlayerNext {
            zone_or_output_id: zone_id.to_string(),
        }).await?;
        Ok(())
    }

    pub async fn previous(&self, zone_id: &str) -> Result<()> {
        self.request(RoonRequest::PlayerPrevious {
            zone_or_output_id: zone_id.to_string(),
        }).await?;
        Ok(())
    }

    pub async fn set_volume(&self, zone_id: &str, volume: u8) -> Result<()> {
        self.request(RoonRequest::PlayerVolume {
            zone_or_output_id: zone_id.to_string(),
            volume,
        }).await?;
        Ok(())
    }

    pub async fn seek(&self, zone_id: &str, position_ms: u64) -> Result<()> {
        self.request(RoonRequest::PlayerSeek {
            zone_or_output_id: zone_id.to_string(),
            seek_position: position_ms,
        }).await?;
        Ok(())
    }

    pub async fn load_config(&self) -> Result<RoonConfig> {
        let config_dir = dirs::config_dir()
            .ok_or_else(|| anyhow!("No config directory"))?
            .join("stui");
        
        let config_path = config_dir.join("roon.json");
        
        if config_path.exists() {
            let content = tokio::fs::read_to_string(&config_path).await?;
            let config: RoonConfig = serde_json::from_str(&content)?;
            *self.config.write().await = config.clone();
            Ok(config)
        } else {
            Ok(RoonConfig::default())
        }
    }

    pub async fn save_config(&self) -> Result<()> {
        let config = self.config.read().await.clone();
        
        let config_dir = dirs::config_dir()
            .ok_or_else(|| anyhow!("No config directory"))?
            .join("stui");
        
        tokio::fs::create_dir_all(&config_dir).await?;
        
        let config_path = config_dir.join("roon.json");
        let content = serde_json::to_string_pretty(&config)?;
        tokio::fs::write(config_path, content).await?;
        
        Ok(())
    }

    pub async fn set_token(&self, core_id: &str, token: String) {
        let mut config = self.config.write().await;
        if let Some(server) = config.servers.iter_mut().find(|s| s.core_id == core_id) {
            server.token = Some(token);
        }
    }
}

impl Default for RoonClient {
    fn default() -> Self {
        Self::new()
    }
}

fn rand_u32() -> u32 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    (nanos % u32::MAX as u128) as u32
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoonSearchResult {
    pub id: String,
    pub title: String,
    pub subtitle: Option<String>,
    pub item_type: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_roon_client_creation() {
        let client = RoonClient::new();
        let _ = client.subscribe();
    }

    #[test]
    fn test_rand_u32() {
        let values: Vec<u32> = (0..100).map(|_| rand_u32()).collect();
        let unique = values.iter().collect::<std::collections::HashSet<_>>().len();
        assert!(unique > 1);
    }
}
