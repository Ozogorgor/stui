pub mod callback_server;
pub mod browser;

pub use callback_server::{OAuthCallback, OAuthReceiver, allocate_port};

use std::time::Duration;

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
    _url: &str,
    _receiver: OAuthReceiver,
    _timeout: Duration,
) -> Result<OAuthCallback, AuthError> {
    todo!("implement in Task 2")
}
