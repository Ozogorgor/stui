//! HTTP transport abstraction for TVDB. Production wires `ReqwestFetcher`;
//! tests will inject a mock impl so cache + dedup behaviour is verified
//! deterministically without standing up a real HTTP server.
//!
//! The TVDB v4 endpoints we use return JSON; both methods give the
//! decoded body string so callers parse with their own envelope types.
//! Status codes are returned to the caller for policy decisions — the
//! fetcher does not classify 4xx/5xx as errors, since "what counts as an
//! error" is TVDB-specific (e.g. 401 triggers a JWT refresh + retry).

use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;

#[async_trait]
pub trait HttpFetch: Send + Sync + 'static {
    /// GET with bearer auth. Returns `Ok(HttpOk { status, body })` for any
    /// HTTP response — caller is responsible for inspecting `status`.
    /// `Err` is reserved for transport failures (timeout, DNS, etc.).
    async fn get_json(&self, url: &str, jwt: &str) -> Result<HttpOk>;

    /// POST a JSON body without auth (for /login). Returns `Ok(HttpOk
    /// { status, body })` for any HTTP response — caller inspects
    /// `status`. `Err` is reserved for transport failures.
    async fn post_json(&self, url: &str, body: &str) -> Result<HttpOk>;
}

/// HTTP response — body string + status code. Returned for any HTTP
/// response regardless of status; callers inspect `status` to apply
/// their own policy (retry on 401, surface 5xx, etc.). The name is
/// historical — "Ok" here means "the transport succeeded", not "the
/// server returned 2xx".
#[derive(Debug)]
pub struct HttpOk {
    pub status: u16,
    pub body: String,
}

/// Production fetcher. Holds a `reqwest::Client` with the same User-Agent
/// + 10s timeout the previous inline implementation used.
pub struct ReqwestFetcher {
    client: reqwest::Client,
}

impl ReqwestFetcher {
    pub fn new(user_agent: &str) -> Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent(user_agent)
            .timeout(Duration::from_secs(10))
            .build()?;
        Ok(Self { client })
    }
}

#[async_trait]
impl HttpFetch for ReqwestFetcher {
    async fn get_json(&self, url: &str, jwt: &str) -> Result<HttpOk> {
        let resp = self
            .client
            .get(url)
            .bearer_auth(jwt)
            .send()
            .await?;
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        Ok(HttpOk { status, body })
    }

    async fn post_json(&self, url: &str, body: &str) -> Result<HttpOk> {
        let resp = self
            .client
            .post(url)
            .header("Content-Type", "application/json")
            .body(body.to_string())
            .send()
            .await?;
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        Ok(HttpOk { status, body })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sanity: trait is object-safe and the production impl satisfies it.
    #[test]
    fn reqwest_fetcher_implements_http_fetch() {
        fn assert_impl<T: HttpFetch>() {}
        assert_impl::<ReqwestFetcher>();
    }
}
