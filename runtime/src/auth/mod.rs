pub mod callback_server;
pub mod browser;

pub use callback_server::{OAuthCallback, OAuthReceiver, allocate_port};

use std::time::Duration;
use tokio::time::timeout;

#[derive(Debug)]
pub enum AuthError {
    TimedOut,
    Denied { message: String },
    BrowserOpenFailed(String),
    ReceiverDropped,
}

/// Opens `url` in the browser then awaits the OAuth callback.
/// Timeout clock starts after the browser launcher returns.
pub async fn open_and_wait(
    url: &str,
    receiver: OAuthReceiver,
    auth_timeout: Duration,
) -> Result<OAuthCallback, AuthError> {
    browser::open_url(url).map_err(AuthError::BrowserOpenFailed)?;

    match timeout(auth_timeout, receiver).await {
        Ok(Ok(cb)) => {
            if let Some(err_msg) = cb.error.clone() {
                return Err(AuthError::Denied { message: err_msg });
            }
            Ok(cb)
        }
        Ok(Err(_)) => Err(AuthError::ReceiverDropped),
        Err(_) => Err(AuthError::TimedOut),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn test_open_and_wait_timeout() {
        let (_port, rx) = allocate_port().await.unwrap();
        // Don't simulate any callback — timeout after 100ms
        let result = open_and_wait(
            "http://example.com/oauth",
            rx,
            Duration::from_millis(100),
        ).await;
        // browser failure or timeout — both acceptable (CI may not have xdg-open)
        assert!(
            matches!(result, Err(AuthError::TimedOut) | Err(AuthError::BrowserOpenFailed(_))),
            "expected TimedOut or BrowserOpenFailed, got {:?}", result
        );
    }

    #[tokio::test]
    async fn test_open_and_wait_denied() {
        // The callback server speaks TLS — we use tokio-rustls with the same
        // self-signed cert embedded in the binary to simulate a browser callback.
        use tokio_rustls::rustls;
        use tokio_rustls::TlsConnector;
        use tokio::io::AsyncWriteExt;

        let (port, rx) = allocate_port().await.unwrap();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(30)).await;

            // Accept the self-signed cert by using a custom verifier that
            // trusts any certificate (suitable only for loopback tests).
            #[derive(Debug)]
            struct AcceptAny;
            impl rustls::client::danger::ServerCertVerifier for AcceptAny {
                fn verify_server_cert(
                    &self, _end_entity: &rustls::pki_types::CertificateDer<'_>,
                    _intermediates: &[rustls::pki_types::CertificateDer<'_>],
                    _server_name: &rustls::pki_types::ServerName<'_>,
                    _ocsp_response: &[u8],
                    _now: rustls::pki_types::UnixTime,
                ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
                    Ok(rustls::client::danger::ServerCertVerified::assertion())
                }
                fn verify_tls12_signature(&self, _: &[u8], _: &rustls::pki_types::CertificateDer<'_>, _: &rustls::DigitallySignedStruct) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
                    Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
                }
                fn verify_tls13_signature(&self, _: &[u8], _: &rustls::pki_types::CertificateDer<'_>, _: &rustls::DigitallySignedStruct) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
                    Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
                }
                fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
                    rustls::crypto::ring::default_provider().signature_verification_algorithms.supported_schemes()
                }
            }

            let tls_cfg = rustls::ClientConfig::builder()
                .dangerous()
                .with_custom_certificate_verifier(std::sync::Arc::new(AcceptAny))
                .with_no_client_auth();

            let connector = TlsConnector::from(std::sync::Arc::new(tls_cfg));
            let tcp = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
            let server_name = rustls::pki_types::ServerName::try_from("localhost").unwrap();
            let mut tls = connector.connect(server_name, tcp).await.unwrap();

            let req = b"GET /callback?error=access_denied HTTP/1.1\r\nHost: localhost\r\n\r\n";
            tls.write_all(req).await.unwrap();
        });
        let result = open_and_wait(
            "http://example.com/oauth",
            rx,
            Duration::from_secs(5),
        ).await;
        match result {
            Err(AuthError::Denied { message }) => assert_eq!(message, "access_denied"),
            Err(AuthError::BrowserOpenFailed(_)) => { /* headless CI — xdg-open absent */ }
            other => panic!("expected Denied or BrowserOpenFailed, got {:?}", other),
        }
    }
}
