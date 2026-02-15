use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

#[derive(Debug, Clone, serde::Serialize)]
struct OpenAiErrorResponse {
    error: OpenAiError,
}

#[derive(Debug, Clone, serde::Serialize)]
struct OpenAiError {
    message: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    r#type: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    code: String,
}

pub(super) fn openai_error(code: StatusCode, msg: &str, typ: &str, api_code: &str) -> Response {
    let body = OpenAiErrorResponse {
        error: OpenAiError {
            message: msg.to_string(),
            r#type: typ.to_string(),
            code: api_code.to_string(),
        },
    };
    (code, Json(body)).into_response()
}

#[derive(Debug, Clone, serde::Serialize)]
struct AuthErrorResponse {
    error: AuthError,
}

#[derive(Debug, Clone, serde::Serialize)]
struct AuthError {
    message: String,
}

pub(super) fn auth_error(code: StatusCode, msg: &str) -> Response {
    (
        code,
        Json(AuthErrorResponse {
            error: AuthError {
                message: msg.to_string(),
            },
        }),
    )
        .into_response()
}
