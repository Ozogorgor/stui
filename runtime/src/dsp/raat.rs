//! Roon RAAT protocol integration.
//!
//! Provides endpoint discovery via `RoonClient` (mDNS + Extension API) and a
//! stub audio-transport layer. Full RAAT TCP framing is not yet implemented —
//! audio writes are accepted and dropped. See SCAFFOLD_TODOS.md.

use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info};

use super::config::DspConfig;
use crate::roon::{RoonClient, RoonServer};

/// RAAT endpoint information.
#[allow(dead_code)] // planned: Roon RAAT audio output, wired in when roon feature is enabled
#[derive(Debug, Clone)]
pub struct RaatEndpoint {
    pub name: String,
    pub device_id: String,
    pub ip_address: String,
    pub port: u16,
    pub sample_rates: Vec<u32>,
    pub bit_depths: Vec<u8>,
    pub is_connected: bool,
    pub token: Option<String>,
}

/// RAAT audio format.
#[allow(dead_code)] // planned: Roon RAAT audio output, wired in when roon feature is enabled
#[derive(Debug, Clone)]
pub struct RaatFormat {
    pub sample_rate: u32,
    pub bit_depth: u8,
    pub channels: u8,
    pub encoding: String,
}

impl Default for RaatFormat {
    fn default() -> Self {
        Self {
            sample_rate: 44100,
            bit_depth: 16,
            channels: 2,
            encoding: "PCM".to_string(),
        }
    }
}

/// RAAT processor for streaming audio to Roon endpoints.
#[allow(dead_code)] // planned: Roon RAAT audio output, wired in when roon feature is enabled
pub struct RaatProcessor {
    config: Arc<RwLock<DspConfig>>,
    roon_client: Option<Arc<RoonClient>>,
    endpoint: Option<RaatEndpoint>,
    active: bool,
    format: RaatFormat,
}

#[allow(dead_code)] // planned: Roon RAAT output integration, wired in when roon feature is enabled
impl RaatProcessor {
    /// Create a new RAAT processor.
    /// Pass a shared `RoonClient` to enable endpoint discovery and connection.
    pub fn new(config: Arc<RwLock<DspConfig>>, roon_client: Option<Arc<RoonClient>>) -> Result<Self, String> {
        let sample_rate = config.blocking_read().output_sample_rate;
        info!(sample_rate = sample_rate, "RAAT processor created");
        Ok(Self {
            config,
            roon_client,
            endpoint: None,
            active: false,
            format: RaatFormat {
                sample_rate,
                ..Default::default()
            },
        })
    }

    /// Discover Roon servers on the local network and return them as RAAT endpoints.
    ///
    /// Requires a `RoonClient` to have been supplied at construction time.
    /// Falls back to an empty list when no client is present.
    pub async fn discover_endpoints(&self) -> Result<Vec<RaatEndpoint>, String> {
        let Some(client) = &self.roon_client else {
            info!("RAAT discovery: no RoonClient configured, returning empty list");
            return Ok(vec![]);
        };

        let servers = client.discover().await.map_err(|e: anyhow::Error| e.to_string())?;
        info!(count = servers.len(), "RAAT endpoint discovery complete");

        Ok(servers.into_iter().map(server_to_endpoint).collect())
    }

    /// Connect to a Roon endpoint via the Extension API, then mark this processor active.
    pub async fn connect(&mut self, endpoint: RaatEndpoint) -> Result<(), String> {
        if self.active {
            return Err("Already connected".to_string());
        }

        let Some(client) = &self.roon_client else {
            return Err("Cannot connect: no RoonClient configured".to_string());
        };

        let server = RoonServer {
            host: endpoint.ip_address.clone(),
            port: endpoint.port,
            core_id: endpoint.device_id.clone(),
            display_name: endpoint.name.clone(),
            token: endpoint.token.clone(),
        };
        client.connect(&server).await.map_err(|e: anyhow::Error| e.to_string())?;

        info!(endpoint = %endpoint.name, ip = %endpoint.ip_address, "connected to RAAT endpoint");
        self.endpoint = Some(endpoint);
        self.active = true;
        Ok(())
    }

    /// Disconnect from current endpoint.
    pub async fn disconnect(&mut self) -> Result<(), String> {
        if !self.active {
            return Ok(());
        }

        // In production: clean up connection, send disconnect message
        
        self.active = false;
        self.endpoint = None;
        
        info!("disconnected from RAAT endpoint");

        Ok(())
    }

    /// Send audio data to endpoint.
    pub async fn send_audio(&mut self, samples: &[f32]) -> Result<(), String> {
        if !self.active {
            return Err("Not connected".to_string());
        }

        // In production, this would encode and send via RAAT protocol
        debug!(samples = samples.len(), "sent audio to RAAT");

        Ok(())
    }

    /// Get current connection status.
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Get current endpoint info.
    pub fn endpoint_info(&self) -> Option<&RaatEndpoint> {
        self.endpoint.as_ref()
    }

    /// Set output format.
    pub fn set_format(&mut self, format: RaatFormat) {
        self.format = format;
    }

    /// Get current format.
    pub fn format(&self) -> &RaatFormat {
        &self.format
    }
}

#[allow(dead_code)] // planned: used by RaatProcessor::discover_endpoints
fn server_to_endpoint(s: RoonServer) -> RaatEndpoint {
    RaatEndpoint {
        name: s.display_name,
        device_id: s.core_id,
        ip_address: s.host,
        port: s.port,
        // TODO: Query actual capabilities from Roon endpoint
        sample_rates: vec![44100, 48000, 88200, 96000, 176400, 192000],
        bit_depths: vec![16, 24, 32],
        is_connected: false,
        token: s.token,
    }
}

/// RAAT protocol encoding types.
#[allow(dead_code)] // planned: Roon RAAT audio output, wired in when roon feature is enabled
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RaatEncoding {
    PCM,
    DSD,
    DSDOverPCM,
}

impl RaatEncoding {
    #[allow(dead_code)] // planned: Roon RAAT audio output, wired in when roon feature is enabled
    pub fn as_str(&self) -> &str {
        match self {
            Self::PCM => "PCM",
            Self::DSD => "DSD",
            Self::DSDOverPCM => "DSD-over-PCM",
        }
    }
}

/// Roon/RAAT integration status.
#[allow(dead_code)] // planned: Roon RAAT audio output, wired in when roon feature is enabled
#[derive(Debug, Clone, Copy, PartialEq)]
#[derive(Default)]
pub enum RaatStatus {
    #[default]
    Disconnected,
    Discovering,
    Connecting,
    Connected,
    Streaming,
    Error,
}


#[cfg(test)]
mod tests {
    use super::*;

    fn make_config() -> Arc<RwLock<DspConfig>> {
        Arc::new(RwLock::new(DspConfig::default()))
    }

    #[test]
    fn test_processor_creation() {
        let config = make_config();
        let processor = RaatProcessor::new(config, None);
        assert!(processor.is_ok());
    }

    #[test]
    fn test_default_format() {
        let format = RaatFormat::default();
        assert_eq!(format.sample_rate, 44100);
        assert_eq!(format.bit_depth, 16);
        assert_eq!(format.channels, 2);
    }

    #[test]
    fn test_status() {
        let status = RaatStatus::default();
        assert_eq!(status, RaatStatus::Disconnected);
    }
}