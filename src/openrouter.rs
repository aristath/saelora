use std::time::Duration;

use bytes::Bytes;
use futures::Stream;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};

pub const DEFAULT_BASE_URL: &str = "https://openrouter.ai/api/v1";

#[derive(Debug, Clone)]
pub struct Client {
    base_url: String,
    api_key: String,
    http_referer: String,
    x_title: String,
    http: reqwest::Client,
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

        Ok(Self {
            base_url,
            api_key,
            http_referer: cfg.http_referer.trim().to_string(),
            x_title: cfg.x_title.trim().to_string(),
            http,
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
            .auth_headers(self.http.post(url))
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<i64>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub stream: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionResponse {
    pub id: String,
    pub object: String,
    pub created: i64,
    pub model: String,
    pub choices: Vec<ChatChoice>,
    #[serde(default)]
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatChoice {
    pub index: i64,
    pub message: Message,
    #[serde(default)]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub prompt_tokens: Option<i64>,
    #[serde(default)]
    pub completion_tokens: Option<i64>,
    #[serde(default)]
    pub total_tokens: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelsResponse {
    pub data: Vec<Model>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Model {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub context_length: i64,
    #[serde(default)]
    pub pricing: Pricing,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Pricing {
    pub prompt: String,
    pub completion: String,
    pub request: String,
    pub image: String,
    pub input_cache_read: String,
    pub input_cache_write: String,
}

impl<'de> Deserialize<'de> for Pricing {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum AnyPricing {
            One(PricingInner),
            Many(Vec<PricingInner>),
        }

        #[derive(Debug, Clone, Serialize, Deserialize, Default)]
        struct PricingInner {
            #[serde(default)]
            prompt: String,
            #[serde(default)]
            completion: String,
            #[serde(default)]
            request: String,
            #[serde(default)]
            image: String,
            #[serde(default)]
            input_cache_read: String,
            #[serde(default)]
            input_cache_write: String,
        }

        let any = AnyPricing::deserialize(deserializer)?;
        let inner = match any {
            AnyPricing::One(p) => p,
            AnyPricing::Many(ps) => ps.into_iter().next().unwrap_or_default(),
        };
        Ok(Pricing {
            prompt: inner.prompt,
            completion: inner.completion,
            request: inner.request,
            image: inner.image,
            input_cache_read: inner.input_cache_read,
            input_cache_write: inner.input_cache_write,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyInfoResponse {
    pub data: KeyInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct KeyInfo {
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub usage: f64,
    #[serde(default)]
    pub limit: f64,
    #[serde(default)]
    pub limit_remaining: f64,
    #[serde(default)]
    pub is_free_tier: bool,
    #[serde(default)]
    pub rate_limit: Option<RateLimit>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RateLimit {
    #[serde(default)]
    pub requests: i64,
    #[serde(default)]
    pub interval: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OpenAiErrorEnvelope {
    pub error: OpenAiError,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OpenAiError {
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub r#type: String,
    #[serde(default)]
    pub code: String,
}

#[derive(Debug, Clone)]
pub struct HttpError {
    pub status: Option<StatusCode>,
    pub message: String,
    pub r#type: String,
    pub code: String,
    pub body: String,
    pub transport: Option<String>,
}

impl HttpError {
    fn client(msg: &str) -> Self {
        Self {
            status: None,
            message: msg.to_string(),
            r#type: String::new(),
            code: String::new(),
            body: String::new(),
            transport: None,
        }
    }

    fn transport<E: std::fmt::Display>(e: E) -> Self {
        Self {
            status: None,
            message: "transport error".to_string(),
            r#type: String::new(),
            code: String::new(),
            body: String::new(),
            transport: Some(e.to_string()),
        }
    }

    fn parse<E: std::fmt::Display>(e: E, body: &[u8]) -> Self {
        Self {
            status: None,
            message: "parse error".to_string(),
            r#type: String::new(),
            code: String::new(),
            body: String::from_utf8_lossy(body).trim().to_string(),
            transport: Some(e.to_string()),
        }
    }

    fn from_api(status: StatusCode, body: &[u8]) -> Self {
        let bs = String::from_utf8_lossy(body).trim().to_string();
        if let Ok(env) = serde_json::from_slice::<OpenAiErrorEnvelope>(body) {
            if !env.error.message.trim().is_empty() {
                return Self {
                    status: Some(status),
                    message: env.error.message,
                    r#type: env.error.r#type,
                    code: env.error.code,
                    body: bs,
                    transport: None,
                };
            }
        }
        Self {
            status: Some(status),
            message: String::new(),
            r#type: String::new(),
            code: String::new(),
            body: bs,
            transport: None,
        }
    }
}

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "openrouter http error status={:?} msg={:?} type={:?} code={:?} transport={:?}",
            self.status, self.message, self.r#type, self.code, self.transport
        )
    }
}

impl std::error::Error for HttpError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pricing_deserializes_from_object_or_array() {
        let one = r#"{
            "id":"m",
            "name":"n",
            "description":"d",
            "context_length":123,
            "pricing":{"prompt":"0.000001","completion":"0.000002","request":"0.01","image":"","input_cache_read":"0.0","input_cache_write":"0.0"}
        }"#;
        let m1: Model = serde_json::from_str(one).unwrap();
        assert_eq!(m1.pricing.prompt, "0.000001");
        assert_eq!(m1.pricing.completion, "0.000002");
        assert_eq!(m1.pricing.request, "0.01");

        let many = r#"{
            "id":"m",
            "name":"n",
            "description":"d",
            "context_length":123,
            "pricing":[{"prompt":"0.1","completion":"0.2","request":"0.3","image":"","input_cache_read":"0.4","input_cache_write":"0.5"}]
        }"#;
        let m2: Model = serde_json::from_str(many).unwrap();
        assert_eq!(m2.pricing.prompt, "0.1");
        assert_eq!(m2.pricing.input_cache_write, "0.5");
    }

    #[test]
    fn http_error_from_api_parses_openai_envelope() {
        let body = br#"{"error":{"message":"no auth","type":"invalid_request_error","code":"unauthorized"}}"#;
        let e = HttpError::from_api(StatusCode::UNAUTHORIZED, body);
        assert_eq!(e.status, Some(StatusCode::UNAUTHORIZED));
        assert_eq!(e.message, "no auth");
        assert_eq!(e.r#type, "invalid_request_error");
        assert_eq!(e.code, "unauthorized");
        assert!(e.transport.is_none());
    }
}
