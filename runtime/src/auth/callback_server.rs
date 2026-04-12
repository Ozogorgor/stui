use percent_encoding::percent_decode_str;

use std::sync::Arc;

use tokio::sync::oneshot;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::ServerConfig;

#[derive(Debug)]
pub struct OAuthCallback {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
}

pub type OAuthReceiver = oneshot::Receiver<OAuthCallback>;

static TLS_CONFIG: std::sync::OnceLock<Arc<ServerConfig>> = std::sync::OnceLock::new();

fn get_tls_config() -> &'static Arc<ServerConfig> {
    TLS_CONFIG.get_or_init(|| {
        // Certificate and key (PEM encoded)
        // Generated with: openssl req -x509 -newkey rsa:2048 -keyout localhost_key.pem -out localhost_cert.pem -days 3650 -nodes -subj "/CN=localhost"
        
        use rustls::pki_types::pem::PemObject;
        
        let cert_pem = include_str!("localhost_cert.pem");
        let key_pem = include_str!("localhost_key.pem");
        
        let cert_der = CertificateDer::pem_slice_iter(cert_pem.as_bytes())
            .collect::<Result<Vec<_>, _>>()
            .expect("auth TLS: failed to parse certificate PEM")
            .pop()
            .expect("auth TLS: failed to generate self-signed cert");
        let key_der = PrivateKeyDer::from_pem_slice(key_pem.as_bytes())
            .expect("auth TLS: failed to parse private key PEM");

        let config = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![cert_der], key_der)
            .expect("auth TLS: failed to configure TLS acceptor");
        
        Arc::new(config)
    })
}

/// Binds a local HTTPS server on a random OS-assigned port (port 0).
/// The server is already listening when this returns.
/// Returns (port, receiver). Server shuts down after one callback
/// or when the receiver is dropped.
pub async fn allocate_port() -> anyhow::Result<(u16, OAuthReceiver)> {
    let tls_config = get_tls_config().clone();
    let acceptor = TlsAcceptor::from(tls_config);
    
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let (tx, rx) = oneshot::channel::<OAuthCallback>();

    tracing::info!("OAuth callback server listening on https://127.0.0.1:{}", port);

    tokio::spawn(async move {
        let tx = Arc::new(tokio::sync::Mutex::new(Some(tx)));
        
        loop {
            tokio::select! {
                result = listener.accept() => {
                    let Ok((stream, _)) = result else { continue };
                    
                    let acceptor = acceptor.clone();
                    let tx = tx.clone();
                    
                    tokio::spawn(async move {
                        let Ok(tls_stream) = acceptor.accept(stream).await else {
                            return;
                        };
                        
                        let mut tls_stream = tls_stream;
                        
                        let mut buf = vec![0u8; 4096];
                        let mut total = 0;
                        loop {
                            if total >= buf.len() { break; }
                            match tls_stream.read(&mut buf[total..]).await {
                                Ok(0) | Err(_) => break,
                                Ok(n) => {
                                    total += n;
                                    if buf[..total].windows(4).any(|w| w == b"\r\n\r\n") {
                                        break;
                                    }
                                }
                            }
                        }
                        
                        let request_line = std::str::from_utf8(&buf[..total])
                            .unwrap_or("")
                            .lines()
                            .next()
                            .unwrap_or("");
                        let qs = request_line
                            .split_once('?')
                            .and_then(|(_, rest)| rest.split_once(' ').map(|(qs, _)| qs))
                            .unwrap_or("");
                        let cb = parse_query(qs);
                        
                        let path = request_line
                            .split_whitespace()
                            .nth(1)
                            .unwrap_or("")
                            .split('?')
                            .next()
                            .unwrap_or("");
                        
                        if path != "/callback" || (cb.code.is_none() && cb.error.is_none()) {
                            let resp = b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                            let _ = tls_stream.write_all(resp).await;
                            return;
                        }
                        
                        let body = b"You may close this tab.";
                        let resp = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = tls_stream.write_all(resp.as_bytes()).await;
                        let _ = tls_stream.write_all(body).await;
                        
                        let mut guard = tx.lock().await;
                        if let Some(tx) = guard.take() {
                            let _ = tx.send(cb);
                        }
                    });
                }
                _ = tokio::time::sleep(std::time::Duration::from_secs(3600)) => {
                    return;
                }
            }
        }
    });

    Ok((port, rx))
}

fn parse_query(qs: &str) -> OAuthCallback {
    let mut code = None;
    let mut state = None;
    let mut error = None;
    for pair in qs.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            let decoded = percent_decode(v);
            match k {
                "code"  => code  = Some(decoded),
                "state" => state = Some(decoded),
                "error" => error = Some(decoded),
                _ => {}
            }
        }
    }
    OAuthCallback { code, state, error }
}

fn percent_decode(s: &str) -> String {
    let plus_replaced = s.replace('+', " ");
    percent_decode_str(&plus_replaced)
        .decode_utf8()
        .map(|cow| cow.into_owned())
        .unwrap_or_else(|_| plus_replaced)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_allocate_returns_port_and_fires_receiver_on_callback() {
        let (port, _rx) = allocate_port().await.unwrap();
        assert!(port > 0, "port must be > 0");
        tracing::info!("Test server running on port {}", port);
    }
}
