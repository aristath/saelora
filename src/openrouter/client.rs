use std::time::Duration;

use bytes::Bytes;
use futures::Stream;
use reqwest::StatusCode;

use super::error::HttpError;
use super::types::{
    ChatCompletionRequest, ChatCompletionResponse, EmbeddingRequest, EmbeddingResponse, KeyInfo,
    KeyInfoResponse, Model, ModelsResponse,
};

pub const DEFAULT_BASE_URL: &str = "https://openrouter.ai/api/v1";

#[derive(Debug, Clone)]
pub struct Client {
    base_url: String,
    api_key: String,
    http_referer: String,
    x_title: String,
    http: reqwest::Client,
    http_stream: reqwest::Client,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub api_key: String,
    pub base_url: String,
    pub http_referer: String,
    pub x_title: String,
}

impl Client {
    pub fn new(cfg: Config) -> anyhow::Result<Self> {
        let api_key = cfg.api_key.trim().to_string();
        let mut base_url = cfg.base_url.trim().trim_end_matches('/').to_string();
        if base_url.is_empty() {
            base_url = DEFAULT_BASE_URL.to_string();
        }

        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()?;
        // Streaming responses can legitimately run much longer than non-stream requests.
        let http_stream = reqwest::Client::builder().build()?;

        Ok(Self {
            base_url,
            api_key,
            http_referer: cfg.http_referer.trim().to_string(),
            x_title: cfg.x_title.trim().to_string(),
            http,
            http_stream,
        })
    }

    fn auth_headers(&self, rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let mut rb = rb.header("Accept", "application/json");
        if !self.api_key.is_empty() {
            rb = rb.bearer_auth(&self.api_key);
        }
        if !self.http_referer.is_empty() {
            rb = rb.header("HTTP-Referer", &self.http_referer);
        }
        if !self.x_title.is_empty() {
            rb = rb.header("X-Title", &self.x_title);
        }
        rb
    }

    pub async fn list_models(&self) -> Result<Vec<Model>, HttpError> {
        let url = format!("{}/models", self.base_url);
        let resp = self
            .auth_headers(self.http.get(url))
            .send()
            .await
            .map_err(HttpError::transport)?;
        let status = resp.status();
        let body = resp.bytes().await.map_err(HttpError::transport)?;
        if !status.is_success() {
            return Err(HttpError::from_api(status, &body));
        }
        let out: ModelsResponse =
            serde_json::from_slice(&body).map_err(|e| HttpError::parse(e, &body))?;
        Ok(out.data)
    }

    pub async fn get_key_info(&self) -> Result<KeyInfo, HttpError> {
        let url = format!("{}/key", self.base_url);
        let resp = self
            .auth_headers(self.http.get(url))
            .send()
            .await
            .map_err(HttpError::transport)?;
        let status = resp.status();
        let body = resp.bytes().await.map_err(HttpError::transport)?;
        if !status.is_success() {
            return Err(HttpError::from_api(status, &body));
        }
        let out: KeyInfoResponse =
            serde_json::from_slice(&body).map_err(|e| HttpError::parse(e, &body))?;
        Ok(out.data)
    }

    pub async fn create_chat_completion(
        &self,
        req: &ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse, HttpError> {
        if req.model.trim().is_empty() {
            return Err(HttpError::client("openrouter: missing model"));
        }
        if req.messages.is_empty() {
            return Err(HttpError::client("openrouter: missing messages"));
        }
        let url = format!("{}/chat/completions", self.base_url);
        let resp = self
            .auth_headers(self.http.post(url))
            .header("Content-Type", "application/json")
            .json(req)
            .send()
            .await
            .map_err(HttpError::transport)?;
        let status = resp.status();
        let body = resp.bytes().await.map_err(HttpError::transport)?;
        if !status.is_success() {
            return Err(HttpError::from_api(status, &body));
        }
        let out: ChatCompletionResponse =
            serde_json::from_slice(&body).map_err(|e| HttpError::parse(e, &body))?;
        Ok(out)
    }

    pub async fn create_embedding(
        &self,
        req: &EmbeddingRequest,
    ) -> Result<EmbeddingResponse, HttpError> {
        if req.model.trim().is_empty() {
            return Err(HttpError::client("openrouter: missing model"));
        }
        if req.input.is_empty() {
            return Err(HttpError::client("openrouter: missing input"));
        }

        let url = format!("{}/embeddings", self.base_url);
        let resp = self
            .auth_headers(self.http.post(url))
            .header("Content-Type", "application/json")
            .json(req)
            .send()
            .await
            .map_err(HttpError::transport)?;
        let status = resp.status();
        let body = resp.bytes().await.map_err(HttpError::transport)?;
        if !status.is_success() {
            return Err(HttpError::from_api(status, &body));
        }
        let out: EmbeddingResponse =
            serde_json::from_slice(&body).map_err(|e| HttpError::parse(e, &body))?;
        Ok(out)
    }

    pub async fn create_chat_completion_stream(
        &self,
        req: &ChatCompletionRequest,
    ) -> Result<
        (
            StatusCode,
            reqwest::header::HeaderMap,
            impl Stream<Item = Result<Bytes, reqwest::Error>>,
        ),
        HttpError,
    > {
        if req.model.trim().is_empty() {
            return Err(HttpError::client("openrouter: missing model"));
        }
        if req.messages.is_empty() {
            return Err(HttpError::client("openrouter: missing messages"));
        }

        let url = format!("{}/chat/completions", self.base_url);
        let mut r = req.clone();
        r.stream = true;
        let resp = self
            .auth_headers(self.http_stream.post(url))
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream")
            .json(&r)
            .send()
            .await
            .map_err(HttpError::transport)?;

        let status = resp.status();
        let headers = resp.headers().clone();
        if !status.is_success() {
            let body = resp.bytes().await.map_err(HttpError::transport)?;
            return Err(HttpError::from_api(status, &body));
        }
        let stream = resp.bytes_stream();
        Ok((status, headers, stream))
    }
}
