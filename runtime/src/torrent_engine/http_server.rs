//! Localhost HTTP server that exposes librqbit's streaming API to mpv.
//!
//! librqbit ships an HTTP API (`librqbit::http_api::HttpApi`) that handles
//! Range requests and serves torrent file bytes at
//! `/torrents/<id>/stream/<file_idx>`. We bind it to `127.0.0.1:0` (kernel
//! picks a free port) so multiple stui instances can coexist, then run the
//! server in a background tokio task. The resolved [`SocketAddr`] is stashed
//! on [`StreamingServer`] so callers can build playable URLs.
//!
//! The serving task lives until process exit; we only return once the
//! listener is bound and we know the port.

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result};
use librqbit::api::Api;
use librqbit::http_api::{HttpApi, HttpApiOptions};
use librqbit::Session;

pub struct StreamingServer {
    pub addr: SocketAddr,
}

impl StreamingServer {
    /// Bind librqbit's HTTP API to `127.0.0.1:0` and spawn it in the
    /// background. Returns once the listener has a port.
    pub async fn spawn(session: Arc<Session>) -> Result<Self> {
        // `tracing-subscriber-utils` feature is enabled in our deps, so
        // `Api::new` takes a third `Option<LineBroadcast>` arg. We don't use
        // log streaming, so pass `None, None`.
        let api = Api::new(session, None, None);
        let http = HttpApi::new(
            api,
            Some(HttpApiOptions {
                read_only: false,
                ..Default::default()
            }),
        );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .context("binding torrent_engine http server to 127.0.0.1:0")?;
        let addr = listener.local_addr().context("reading listener addr")?;

        // `make_http_api_and_run` consumes `self`, takes a `TcpListener` and
        // an optional `axum::Router` for nested upnp routes (we have none).
        // It returns a `BoxFuture<'static, anyhow::Result<()>>` that runs
        // until the server stops.
        tokio::spawn(async move {
            if let Err(e) = http.make_http_api_and_run(listener, None).await {
                tracing::error!("torrent_engine http server died: {e:#}");
            }
        });

        Ok(Self { addr })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn server_binds_to_localhost() {
        let tmp = tempfile::tempdir().unwrap();
        let session = Session::new(tmp.path().to_path_buf()).await.unwrap();
        let server = StreamingServer::spawn(session).await.unwrap();
        assert!(server.addr.ip().is_loopback());
        assert!(server.addr.port() > 0);
    }
}
