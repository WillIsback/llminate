//! Thin async HTTP client for OpenAI-compatible servers (vLLM, SGLang).

use crate::config::ResolvedTarget;
use crate::error::{Error, Result};
use reqwest::{Method, RequestBuilder, Response};
use serde_json::Value;
use std::time::Duration;

/// HTTP client bound to one resolved target.
#[derive(Debug, Clone)]
pub struct Client {
    http: reqwest::Client,
    base: String,
    api_key: Option<String>,
}

impl Client {
    /// Build a client for `target`.
    pub fn new(target: &ResolvedTarget) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(target.timeout))
            .user_agent(concat!("llminate/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self {
            http,
            base: target.url.trim_end_matches('/').to_string(),
            api_key: target.api_key.clone(),
        })
    }

    /// Base URL without a trailing slash.
    pub fn base_url(&self) -> &str {
        &self.base
    }

    /// Whether a non-empty API key is configured.
    pub fn has_api_key(&self) -> bool {
        self.api_key
            .as_deref()
            .map(|k| !k.is_empty())
            .unwrap_or(false)
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    fn builder(&self, method: Method, path: &str) -> RequestBuilder {
        let mut rb = self.http.request(method, self.url(path));
        if let Some(k) = &self.api_key {
            rb = rb.bearer_auth(k);
        }
        rb
    }

    /// GET a path, returning the body on 2xx and an error otherwise.
    pub async fn get(&self, path: &str) -> Result<String> {
        self.send(Method::GET, path, None).await
    }

    /// GET without the `Authorization` header (used for unauthenticated `/metrics`).
    pub async fn get_unauthenticated(&self, path: &str) -> Result<String> {
        let rb = self.http.request(Method::GET, self.url(path));
        Self::finish(rb, path).await
    }

    /// GET a path, returning `None` on any failure (best-effort probes).
    pub async fn get_opt(&self, path: &str) -> Option<String> {
        self.get(path).await.ok()
    }

    /// POST a JSON body, returning the response body on 2xx.
    pub async fn post_json(&self, path: &str, body: &Value) -> Result<String> {
        self.send(Method::POST, path, Some(body)).await
    }

    /// POST a JSON body and hand back the streaming response (for SSE).
    pub async fn post_stream(&self, path: &str, body: &Value) -> Result<Response> {
        let resp = self.builder(Method::POST, path).json(body).send().await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(Error::Http {
                status: status.as_u16(),
                path: path.to_string(),
                body: truncate(&body),
            });
        }
        Ok(resp)
    }

    async fn send(&self, method: Method, path: &str, body: Option<&Value>) -> Result<String> {
        let mut rb = self.builder(method, path);
        if let Some(b) = body {
            rb = rb.json(b);
        }
        Self::finish(rb, path).await
    }

    async fn finish(rb: RequestBuilder, path: &str) -> Result<String> {
        let resp = rb.send().await?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            return Err(Error::Http {
                status: status.as_u16(),
                path: path.to_string(),
                body: truncate(&text),
            });
        }
        Ok(text)
    }
}

fn truncate(s: &str) -> String {
    const MAX: usize = 500;
    let s = s.trim();
    if s.chars().count() <= MAX {
        return s.to_string();
    }
    let head: String = s.chars().take(MAX).collect();
    format!("{head}…")
}
