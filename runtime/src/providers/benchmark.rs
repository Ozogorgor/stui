//! Stream benchmarking — measures HTTP(S) stream throughput and latency.
//!
//! Probes HTTP URLs with a Range request to estimate download speed.
//! Non-HTTP URLs (magnet:, .torrent) return None for benchmarking data,
//! allowing callers to fall back to seeder-count estimation.

use std::time::{Duration, Instant};
use std::sync::Arc;

use tokio::sync::Semaphore;
use futures_util::StreamExt;

use crate::providers::Stream;

const PROBE_SIZE: usize = 64 * 1024; // 64 KB
const PROBE_TIMEOUT: Duration = Duration::from_secs(8);
const CONCURRENCY_LIMIT: usize = 8;

/// Result of probing a single stream URL.
#[derive(Debug, Clone)]
pub struct ProbeResult {
    pub url: String,
    pub speed_mbps: Option<f64>,
    pub latency_ms: Option<u32>,
    pub error: Option<String>,
}

/// Stream benchmarker — probes HTTP(S) streams to measure throughput.
pub struct StreamBenchmarker {
    client: reqwest::Client,
    concurrency: usize,
}

impl StreamBenchmarker {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(PROBE_TIMEOUT)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self {
            client,
            concurrency: CONCURRENCY_LIMIT,
        }
    }

    pub fn with_concurrency(mut self, limit: usize) -> Self {
        self.concurrency = limit;
        self
    }

    /// Probe a single URL and return the result.
    pub async fn probe(&self, url: &str) -> ProbeResult {
        let url_lower = url.to_lowercase();
        if !url_lower.starts_with("http://") && !url_lower.starts_with("https://") {
            return ProbeResult {
                url: url.to_string(),
                speed_mbps: None,
                latency_ms: None,
                error: Some("not an HTTP stream".to_string()),
            };
        }

        let request = match self.client.get(url).build() {
            Ok(req) => req,
            Err(e) => {
                return ProbeResult {
                    url: url.to_string(),
                    speed_mbps: None,
                    latency_ms: None,
                    error: Some(format!("failed to build request: {}", e)),
                }
            }
        };

        let start = Instant::now();
        let latency_ms = Some(start.elapsed().as_millis() as u32);

        match self.probe_inner(&request, start).await {
            Ok((bytes, elapsed)) => {
                let speed_bps = bytes as f64 / elapsed.as_secs_f64();
                let speed_mbps = speed_bps * 8.0 / 1_000_000.0;

                ProbeResult {
                    url: url.to_string(),
                    speed_mbps: Some(speed_mbps),
                    latency_ms,
                    error: None,
                }
            }
            Err(e) => ProbeResult {
                url: url.to_string(),
                speed_mbps: None,
                latency_ms,
                error: Some(e),
            },
        }
    }

    async fn probe_inner(
        &self,
        request: &reqwest::Request,
        start: Instant,
    ) -> Result<(usize, Duration), String> {
        let req = request.try_clone().ok_or_else(|| "failed to clone request".to_string())?;
        let response = self
            .client
            .execute(req)
            .await
            .map_err(|e| format!("request failed: {}", e))?;

        let mut stream = response.bytes_stream();
        let mut total_bytes = 0usize;
        let mut last_read = Instant::now();

        while let Some(chunk) = stream.next().await {
            let chunk = match chunk {
                Ok(c) => c,
                Err(e) => {
                    if total_bytes == 0 {
                        return Err(format!("failed to read body: {}", e));
                    }
                    break;
                }
            };

            total_bytes += chunk.len();

            if total_bytes >= PROBE_SIZE {
                break;
            }

            let now = Instant::now();
            if now.duration_since(last_read).as_secs() >= 2 {
                break;
            }
            last_read = now;
        }

        let elapsed = start.elapsed();
        if elapsed.as_secs() == 0 && total_bytes == 0 {
            return Err("no data received".to_string());
        }

        Ok((total_bytes, elapsed))
    }

    /// Probe multiple streams concurrently, respecting the concurrency limit.
    pub async fn probe_all(&self, streams: &[Stream]) -> Vec<Stream> {
        let sem = Arc::new(Semaphore::new(self.concurrency));
        let bench = self.clone();

        let handles: Vec<_> = streams
            .iter()
            .map(|stream| {
                let sem = Arc::clone(&sem);
                let stream = stream.clone();
                let bench = bench.clone();

                tokio::spawn(async move {
                    let _permit = sem.acquire().await.expect("benchmark semaphore closed unexpectedly");
                    let result = bench.probe(&stream.url).await;

                    let mut probed = stream.clone();
                    probed.speed_mbps = result.speed_mbps;
                    probed.latency_ms = result.latency_ms;

                    probed
                })
            })
            .collect();

        let mut results = Vec::with_capacity(streams.len());
        for handle in handles {
            if let Ok(probed) = handle.await {
                results.push(probed);
            }
        }

        // Preserve original ordering
        let original_order: Vec<_> = streams.iter().map(|s| s.id.clone()).collect();
        results.sort_by_key(|s| {
            original_order
                .iter()
                .position(|id| id == &s.id)
                .unwrap_or(usize::MAX)
        });

        results
    }

    /// Estimate speed for torrent streams based on seeder count.
    /// Uses a rough heuristic: 100 seeders ≈ 12 Mbps.
    pub fn estimate_torrent_speed(seeders: Option<u32>) -> Option<f64> {
        seeders.map(|s| s as f64 * 0.12)
    }
}

impl Default for StreamBenchmarker {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for StreamBenchmarker {
    fn clone(&self) -> Self {
        Self {
            client: self.client.clone(),
            concurrency: self.concurrency,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_stream(url: &str) -> Stream {
        Stream {
            id: url.to_string(),
            name: "Test Stream".to_string(),
            url: url.to_string(),
            mime: None,
            quality: crate::providers::StreamQuality::Hd1080,
            provider: "test".to_string(),
            protocol: None,
            seeders: None,
            bitrate_kbps: None,
            codec: None,
            resolution: None,
            hdr: crate::providers::HdrFormat::None,
            size_bytes: None,
            latency_ms: None,
            speed_mbps: None,
            audio_channels: None,
            language: None,
        }
    }

    #[tokio::test]
    async fn probe_rejects_non_http_urls() {
        let bench = StreamBenchmarker::new();
        
        let result = bench.probe("magnet:?xt=urn:btih:1234567890").await;
        assert!(result.speed_mbps.is_none());
        assert!(result.error.is_some());
        assert!(result.error.unwrap().contains("not an HTTP stream"));
    }

    #[tokio::test]
    async fn probe_all_preserves_order() {
        let bench = StreamBenchmarker::new();
        
        let streams: Vec<Stream> = vec![
            make_stream("http://example.com/stream1"),
            make_stream("http://example.com/stream2"),
            make_stream("http://example.com/stream3"),
        ];
        
        let results = bench.probe_all(&streams).await;
        
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].id, "http://example.com/stream1");
        assert_eq!(results[1].id, "http://example.com/stream2");
        assert_eq!(results[2].id, "http://example.com/stream3");
    }

    #[tokio::test]
    async fn probe_all_handles_mixed_urls() {
        let bench = StreamBenchmarker::new();
        
        let streams: Vec<Stream> = vec![
            make_stream("http://example.com/stream1"),
            make_stream("magnet:?xt=urn:btih:nothttp"),
            make_stream("http://example.com/stream3"),
        ];
        
        let results = bench.probe_all(&streams).await;
        
        assert_eq!(results.len(), 3);
        assert_eq!(results[1].url, "magnet:?xt=urn:btih:nothttp");
        assert!(results[1].speed_mbps.is_none());
    }

    #[test]
    fn estimate_torrent_speed_zero_seeders() {
        let speed = StreamBenchmarker::estimate_torrent_speed(Some(0));
        assert_eq!(speed, Some(0.0));
    }

    #[test]
    fn estimate_torrent_speed_100_seeders() {
        let speed = StreamBenchmarker::estimate_torrent_speed(Some(100));
        assert_eq!(speed, Some(12.0));
    }

    #[test]
    fn estimate_torrent_speed_none() {
        let speed = StreamBenchmarker::estimate_torrent_speed(None);
        assert!(speed.is_none());
    }

    #[test]
    fn estimate_torrent_speed_scales_linearly() {
        let speed_50 = StreamBenchmarker::estimate_torrent_speed(Some(50));
        let speed_200 = StreamBenchmarker::estimate_torrent_speed(Some(200));
        
        assert_eq!(speed_50, Some(6.0));
        assert_eq!(speed_200, Some(24.0));
    }

    #[tokio::test]
    async fn probe_all_empty_input() {
        let bench = StreamBenchmarker::new();
        let results = bench.probe_all(&[]).await;
        assert!(results.is_empty());
    }
}
