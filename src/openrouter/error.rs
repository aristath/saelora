use reqwest::StatusCode;
use serde::{Deserialize, Serialize};

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
    pub(super) fn client(msg: &str) -> Self {
        Self {
            status: None,
            message: msg.to_string(),
            r#type: String::new(),
            code: String::new(),
            body: String::new(),
            transport: None,
        }
    }

    pub(super) fn transport<E: std::fmt::Display>(e: E) -> Self {
        Self {
            status: None,
            message: "transport error".to_string(),
            r#type: String::new(),
            code: String::new(),
            body: String::new(),
            transport: Some(e.to_string()),
        }
    }

    pub(super) fn parse<E: std::fmt::Display>(e: E, body: &[u8]) -> Self {
        Self {
            status: None,
            message: "parse error".to_string(),
            r#type: String::new(),
            code: String::new(),
            body: String::from_utf8_lossy(body).trim().to_string(),
            transport: Some(e.to_string()),
        }
    }

    pub(super) fn from_api(status: StatusCode, body: &[u8]) -> Self {
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
