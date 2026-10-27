//! Shared HTTP plumbing for every Teams front door.
//!
//! One place implements what the endpoint table in `docs/endpoints.md` calls convention:
//! the Teams-like User-Agent, the per-service auth header (`Authentication: skypetoken=…`
//! vs `Authorization: Bearer …`), an `MS-CV` correlation vector on **every** request, and
//! the retry loop (jittered exponential backoff honouring `Retry-After`) for throttling
//! and transient front-door failures. A **401** is neither: it evicts and re-mints the
//! credential through a [`crate::TokenRefresher`] and replays the request exactly once
//! (the `refresh` module). Everything else — URL building, payload shape, typed
//! parsing — stays with the client that owns the endpoint.
//!
//! Auth credentials are *never* logged and never appear in `Debug` output: the redacting
//! `Debug` impl below is deliberate.

use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use serde_json::Value;
use url::Url;

use crate::refresh::TokenRefresher;
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
    /// Auth header *name* (`Authentication` / `Authorization`).
    auth_name: &'static str,
    /// Auth header *value* — a credential, so it is never logged and never in `Debug`.
    ///
    /// Interior mutability: a 401 handler swaps in a re-minted value while the request
    /// methods only take `&self`. The lock guards short clones, never an await.
    auth_value: Mutex<String>,
    /// 401 recovery hook; `None` = surface 401 to the caller (one-shot callers, tests).
    refresher: Option<Box<dyn TokenRefresher>>,
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
            auth_name: auth_header.0,
            auth_value: Mutex::new(auth_header.1),
            refresher: None,
            extra_headers,
        }
    }

    /// Override the retry schedule (tests use short delays; callers may tune).
    pub(crate) fn with_retry_policy(mut self, policy: RetryPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Attach the 401 recovery hook (evict → re-mint → replay once).
    pub(crate) fn with_refresher(mut self, refresher: impl TokenRefresher + 'static) -> Self {
        self.refresher = Some(Box::new(refresher));
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

    /// Send one logical request, retrying retryable statuses within the policy budget
    /// and re-minting a rejected credential exactly once.
    async fn send(&self, url: Url, body: Body) -> Result<Value, ApiError> {
        let mut attempt = 1u32;
        // Set when the 401 recovery ran: at most one re-mint + replay per request.
        let mut refreshed = false;

        loop {
            let cv = self.cv.next();
            // Snapshot the credential for this attempt (the 401 path compares against it).
            let auth_value = self.current_auth_value();
            let mut builder = match &body {
                Body::None => self.http.get(url.clone()),
                Body::Json(value) => self.http.post(url.clone()).json(value),
            };
            builder = builder
                .header(self.auth_name, auth_value.as_str())
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

            // 401 = "this credential is dead", not "slow down": backoff must not touch it
            // (see retry.rs — 401 is excluded from the retryable statuses). Instead evict
            // → re-mint through the refresher and replay exactly once; if the service
            // rejects the fresh credential too, `refreshed` sends it to the error path.
            if status.as_u16() == 401 && !refreshed && self.refresher.is_some() {
                tracing::warn!(
                    status = 401,
                    cv = %cv,
                    response_cv = response_cv.as_deref().unwrap_or("-"),
                    "credential rejected — re-minting, replaying once"
                );
                self.rearm_credential(&auth_value).await?;
                refreshed = true;
                continue;
            }

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

    /// Current auth header value — a short clone under the lock (never held across await).
    fn current_auth_value(&self) -> String {
        self.auth_value.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// 401 recovery: ask the refresher for a fresh credential and swap it in.
    ///
    /// The lock is never held across the await — snapshot first, refresh unlocked, then
    /// compare-and-swap: if a concurrent request re-minted while we waited, its value is
    /// no longer `stale` and stays (that request's redemption is the fresher one).
    async fn rearm_credential(&self, stale: &str) -> Result<(), ApiError> {
        let Some(refresher) = &self.refresher else {
            return Ok(()); // no hook attached: caller surfaces the 401
        };
        if self.current_auth_value().as_str() != stale {
            return Ok(()); // already re-minted by a concurrent request
        }
        let fresh = refresher.refresh().await?;
        let mut current = self.auth_value.lock().unwrap_or_else(PoisonError::into_inner);
        if current.as_str() == stale {
            *current = fresh;
        }
        Ok(())
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
            .field("auth_name", &self.auth_name)
            .field("refresher", &self.refresher.is_some())
            .field("extra_headers", &self.extra_headers.len())
            .field("retry", &self.policy)
            .field("cv_base", &self.cv.base())
            .finish_non_exhaustive()
    }
}
