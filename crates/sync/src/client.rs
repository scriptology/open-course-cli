//! HTTP client: device-flow auth, `/v1/me`, and the request plumbing
//! (timeouts, bearer auth, retry with exponential backoff) shared by push
//! and pull.

use std::time::Duration;

use reqwest::{RequestBuilder, Response, StatusCode};

use crate::error::SyncError;
use crate::protocol::{DeviceCodeResponse, ErrorBody, MeResponse, TokenSet};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// Short timeout for the pull-on-start path: sync must never delay startup.
const PULL_ON_START_TIMEOUT: Duration = Duration::from_millis(2500);

const MAX_ATTEMPTS: usize = 3;
const RETRY_BASE_DELAY: Duration = Duration::from_millis(100);

/// Outcome of one device-flow poll.
#[derive(Debug, Clone)]
pub enum PollResult {
    /// The user has not authorized the device yet; keep polling.
    Pending,
    /// The device code expired; restart the flow.
    Expired,
    Authorized(TokenSet),
}

pub struct SyncClient {
    http: reqwest::Client,
    http_short: reqwest::Client,
    base_url: String,
    access_token: Option<String>,
}

impl SyncClient {
    pub fn new(base_url: impl Into<String>) -> Result<Self, SyncError> {
        let http = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()?;
        let http_short = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(PULL_ON_START_TIMEOUT)
            .build()?;
        Ok(Self {
            http,
            http_short,
            base_url: base_url.into().trim_end_matches('/').to_string(),
            access_token: None,
        })
    }

    pub fn with_access_token(mut self, token: impl Into<String>) -> Self {
        self.access_token = Some(token.into());
        self
    }

    pub fn set_access_token(&mut self, token: Option<String>) {
        self.access_token = token;
    }

    pub(crate) fn http_ref(&self) -> &reqwest::Client {
        &self.http
    }

    pub(crate) fn http_short_ref(&self) -> &reqwest::Client {
        &self.http_short
    }

    /// `POST {base}/auth/device` — start the device authorization flow.
    pub async fn start_device_flow(&self) -> Result<DeviceCodeResponse, SyncError> {
        let resp = self
            .send_with_retry(|| self.http.post(self.url("/auth/device")))
            .await?;
        let resp = check_status(resp).await?;
        Ok(resp.json().await?)
    }

    /// `POST {base}/auth/device/poll` — one poll of the device flow.
    /// Authorized → 200; pending → 428 with an error body; expired → 400
    /// with an error body. The bare error-field style (any status) is
    /// accepted too.
    pub async fn poll_device_flow(&self, device_code: &str) -> Result<PollResult, SyncError> {
        let resp = self
            .http
            .post(self.url("/auth/device/poll"))
            .json(&serde_json::json!({ "deviceCode": device_code }))
            .send()
            .await?;
        let status = resp.status();
        let body = resp.text().await?;
        if status.is_success()
            && let Ok(tokens) = serde_json::from_str::<TokenSet>(&body)
        {
            return Ok(PollResult::Authorized(tokens));
        }
        let error = serde_json::from_str::<ErrorBody>(&body)
            .map(|b| b.error)
            .unwrap_or_default();
        if status == StatusCode::from_u16(428).unwrap() || error == "authorization_pending" {
            return Ok(PollResult::Pending);
        }
        if error == "expired_token" || status == StatusCode::GONE {
            return Ok(PollResult::Expired);
        }
        if status == StatusCode::UNAUTHORIZED {
            return Err(SyncError::Unauthorized);
        }
        Err(SyncError::Server(format!(
            "device poll failed with {status}: {body}"
        )))
    }

    /// `GET {base}/v1/me` — the authenticated user's profile.
    pub async fn me(&self) -> Result<MeResponse, SyncError> {
        let resp = self
            .send_with_retry(|| self.authorized(self.http.get(self.url("/v1/me"))))
            .await?;
        let resp = check_status(resp).await?;
        Ok(resp.json().await?)
    }

    pub(crate) fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    pub(crate) fn authorized(&self, req: RequestBuilder) -> RequestBuilder {
        match &self.access_token {
            Some(token) => req.bearer_auth(token),
            None => req,
        }
    }

    /// Sends a request, retrying network errors and 5xx responses with
    /// exponential backoff (`MAX_ATTEMPTS` total attempts). 4xx responses
    /// are returned immediately for the caller to interpret.
    pub(crate) async fn send_with_retry(
        &self,
        build: impl Fn() -> RequestBuilder,
    ) -> Result<Response, SyncError> {
        let mut attempt = 0usize;
        loop {
            attempt += 1;
            let result = build().send().await;
            let retryable = match &result {
                Ok(resp) => resp.status().is_server_error(),
                Err(e) => e.is_connect() || e.is_timeout() || e.is_request(),
            };
            if retryable && attempt < MAX_ATTEMPTS {
                tokio::time::sleep(RETRY_BASE_DELAY * (1 << (attempt - 1) as u32)).await;
                continue;
            }
            return Ok(result?);
        }
    }
}

/// Maps 401 to `Unauthorized`, 5xx (after retries) to `Server`; passes
/// through success and other 4xx statuses for the caller.
pub(crate) async fn check_status(resp: Response) -> Result<Response, SyncError> {
    let status = resp.status();
    if status == StatusCode::UNAUTHORIZED {
        return Err(SyncError::Unauthorized);
    }
    if status.is_server_error() {
        let body = resp.text().await.unwrap_or_default();
        return Err(SyncError::Server(format!(
            "server responded {status}: {body}"
        )));
    }
    Ok(resp)
}
