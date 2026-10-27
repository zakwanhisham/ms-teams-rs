//! Shared HTTP plumbing for every Teams front door.
//!
//! One place implements what the endpoint table in `docs/endpoints.md` calls convention:
//! the Teams-like User-Agent, the per-service auth header (`Authentication: skypetoken=…`
//! vs `Authorization: Bearer …`), an `MS-CV` correlation vector on **every** request, and
//! the retry loop (jittered exponential backoff honouring `Retry-After`) for throttling
//! and transient front-door failures. Everything else — URL building, payload shape,
//! typed parsing — stays with the client that owns the endpoint.
//!
//! Auth credentials are *never* logged and never appear in `Debug` output: the redacting
//! `Debug` impl below is deliberate.

use std::time::Duration;

use serde_json::Value;
use url::Url;

use crate::retry::RetryPolicy;
use crate::{ApiError, CorrelationVector, truncate};

/// Request method of one attempt. Cloned per attempt because a
/// [`reqwest::RequestBuilder`] is consumed by `.send()`.
#[derive(Debug, Clone)]
enum Body {
    /// Plain read.
    None,
    /// JSON payload (people search) — replayed verbatim on every attempt.
    Json(Value),
}

/// Auth header plus static service headers of one client.
pub(crate) struct HttpService {
    http: reqwest::Client,
    /// Fresh correlation vector component per request.
    cv: CorrelationVector,
    policy: RetryPolicy,
    /// `("Authentication" | "Authorization", value)` — the value is a credential and
    /// must never be logged or printed.
    auth_header: (&'static str, String),
    /// Static per-service headers, e.g. substrate's mandatory `Origin`/`Referer`.
    extra_headers: &'static [(&'static str, &'static str)],
}

impl HttpService {
    /// Wrap a client for one service: shared HTTP client, its auth header, and any
    /// static headers the front door insists on.
    pub(crate) fn new(
        http: reqwest::Client,
        auth_header: (&'static str, String),
        extra_headers: &'static [(&'static str, &'static str)],
    ) -> Self {
        Self {
            http,
            cv: CorrelationVector::new(),
            policy: RetryPolicy::default(),
            auth_header,
            extra_headers,
        }
    }

    /// Override the retry schedule (tests use short delays; callers may tune).
    pub(crate) fn with_retry_policy(mut self, policy: RetryPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// `GET {url}` and parse the JSON body.
    ///
    /// # Errors
    ///
    /// [`ApiError::Status`] on a non-2xx that is not retryable (or that survived the
    /// retry budget), [`ApiError::Json`] on a malformed body, [`ApiError::Http`] on
    /// transport failure.
    pub(crate) async fn get_json(&self, url: Url) -> Result<Value, ApiError> {
        self.send(url, Body::None).await
    }

    /// `POST {url}` with a JSON body and parse the JSON response.
    ///
    /// # Errors
    ///
    /// Same as [`HttpService::get_json`].
    pub(crate) async fn post_json(&self, url: Url, body: &Value) -> Result<Value, ApiError> {
        self.send(url, Body::Json(body.clone())).await
    }

    /// Send one logical request, retrying retryable statuses within the policy budget.
    async fn send(&self, url: Url, body: Body) -> Result<Value, ApiError> {
        let mut attempt = 1u32;

        loop {
            let cv = self.cv.next();
            let mut builder = match &body {
                Body::None => self.http.get(url.clone()),
                Body::Json(value) => self.http.post(url.clone()).json(value),
            };
            builder = builder
                .header(self.auth_header.0, self.auth_header.1.as_str())
                .header(crate::mscv::MS_CV_HEADER, cv.as_str())
                .header("Accept", "application/json");
            for (name, value) in self.extra_headers {
                builder = builder.header(*name, *value);
            }

            let response = builder.send().await.map_err(|err| {
                // Transport failures are intentionally *not* retried: once the write path
                // (Phase 5) posts messages over this plumbing, a blind transport retry
                // could duplicate a send. Callers retry explicitly instead.
                tracing::warn!(%err, cv = %cv, "transport failure");
                ApiError::Http(err)
            })?;

            // The service extends our vector in its response; keep it for the log line.
            let response_cv = response
                .headers()
                .get(crate::mscv::MS_CV_HEADER)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);

            let status = response.status();
            if status.is_success() {
                let text = response.text().await?;
                tracing::debug!(
                    cv = %cv,
                    response_cv = response_cv.as_deref().unwrap_or("-"),
                    "ok"
                );
                return Ok(serde_json::from_str(&text)?);
            }

            let retry_after = retry_after_from(&response);
            let text = response.text().await?;

            if RetryPolicy::is_retryable(status.as_u16()) && attempt < self.policy.max_attempts {
                let delay = self.policy.delay(attempt, retry_after);
                tracing::warn!(
                    status = status.as_u16(),
                    attempt,
                    delay = ?delay,
                    cv = %cv,
                    "retrying after service error"
                );
                tokio::time::sleep(delay).await;
                attempt += 1;
                continue;
            }

            // Never log the body verbatim: service errors can echo request context.
            tracing::warn!(
                status = status.as_u16(),
                body_len = text.len(),
                cv = %cv,
                response_cv = response_cv.as_deref().unwrap_or("-"),
                "service error"
            );
            return Err(ApiError::Status { status: status.as_u16(), body: truncate(&text, 300) });
        }
    }
}

/// `Retry-After` of a response, parsed as a duration (delta-seconds or HTTP-date).
fn retry_after_from(response: &reqwest::Response) -> Option<Duration> {
    let value = response.headers().get(reqwest::header::RETRY_AFTER)?.to_str().ok()?;
    crate::retry::parse_retry_after(value, chrono::Utc::now())
}

/// The shared HTTP client: Teams-like User-Agent (some front doors reject unknown
/// agents) and a 30 s ceiling per attempt.
///
/// # Errors
///
/// [`ApiError::Http`] when the client cannot be built (invalid TLS roots).
pub(crate) fn build_http_client() -> Result<reqwest::Client, ApiError> {
    Ok(reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent(crate::USER_AGENT)
        .build()?)
}

/// Credentials must never leak through `Debug` — only the header *name* is shown.
impl std::fmt::Debug for HttpService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpService")
            .field("auth_header", &self.auth_header.0)
            .field("extra_headers", &self.extra_headers.len())
            .field("retry", &self.policy)
            .field("cv_base", &self.cv.base())
            .finish_non_exhaustive()
    }
}
