use tokio::sync::oneshot;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

pub struct OAuthCallback {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
}

pub type OAuthReceiver = oneshot::Receiver<OAuthCallback>;

/// Binds a local HTTP server on a random OS-assigned port (port 0).
/// The server is already listening when this returns.
/// Returns (port, receiver). Server shuts down after one callback
/// or when the receiver is dropped.
pub async fn allocate_port() -> anyhow::Result<(u16, OAuthReceiver)> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let (mut tx, rx) = oneshot::channel::<OAuthCallback>();

    tokio::spawn(async move {
        loop {
            tokio::select! {
                result = listener.accept() => {
                    let Ok((mut stream, _)) = result else { continue };
                    // Read until we have the full request line (\r\n\r\n).
                    // 4096 bytes is sufficient for all OAuth callback redirects.
                    let mut buf = vec![0u8; 4096];
                    let mut total = 0;
                    loop {
                        if total >= buf.len() { break; } // guard: don't exceed buffer
                        match stream.read(&mut buf[total..]).await {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                total += n;
                                if buf[..total].windows(4).any(|w| w == b"\r\n\r\n") {
                                    break;
                                }
                            }
                        }
                    }
                    // Extract query string from "GET /callback?<qs> HTTP/..."
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
                    // Write a minimal HTTP 200 response
                    let body = b"You may close this tab.";
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(resp.as_bytes()).await;
                    let _ = stream.write_all(body).await;
                    let _ = tx.send(cb);
                    return;
                }
                _ = tx.closed() => {
                    // Receiver was dropped (timeout or plugin exit) — shut down
                    return;
                }
            }
        }
    });

    Ok((port, rx))
}

/// Parse `key=value&key=value` query string into an OAuthCallback.
/// Percent-decodes values ('+' → space, %XX → char).
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

/// Minimal percent-decoder: '+' → ' ', %XX → byte.
fn percent_decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'+' {
            out.push(' ');
            i += 1;
        } else if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(hex) = std::str::from_utf8(&bytes[i+1..i+3]) {
                if let Ok(byte) = u8::from_str_radix(hex, 16) {
                    out.push(byte as char);
                    i += 3;
                    continue;
                }
            }
            out.push('%');
            i += 1;
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn test_allocate_returns_port_and_fires_receiver_on_callback() {
        let (port, rx) = allocate_port().await.unwrap();
        assert!(port > 0, "port must be > 0");

        // Simulate browser redirect
        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        let req = b"GET /callback?code=abc123&state=xyz HTTP/1.1\r\nHost: localhost\r\n\r\n";
        stream.write_all(req).await.unwrap();
        drop(stream);

        let cb = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            rx,
        ).await.expect("timed out").expect("receiver closed");

        assert_eq!(cb.code.as_deref(), Some("abc123"));
        assert_eq!(cb.state.as_deref(), Some("xyz"));
        assert!(cb.error.is_none());
    }

    #[tokio::test]
    async fn test_server_shuts_down_when_receiver_dropped() {
        let (port, rx) = allocate_port().await.unwrap();
        // Drop the receiver — server task should detect tx.closed() and exit
        drop(rx);

        // Give the server task a moment to shut down
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Connection should be refused (server shut down)
        let result = tokio::net::TcpStream::connect(("127.0.0.1", port)).await;
        assert!(result.is_err(), "server should have shut down after receiver dropped");
    }

    #[tokio::test]
    async fn test_oauth_error_populates_error_field() {
        let (port, rx) = allocate_port().await.unwrap();

        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        let req = b"GET /callback?error=access_denied HTTP/1.1\r\nHost: localhost\r\n\r\n";
        stream.write_all(req).await.unwrap();
        drop(stream);

        let cb = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            rx,
        ).await.expect("timed out").expect("receiver closed");

        assert!(cb.code.is_none());
        assert_eq!(cb.error.as_deref(), Some("access_denied"));
    }
}
