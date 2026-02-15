use serde::Serialize;
use tracing::warn;

use crate::config;

const MAILJET_DEFAULT_ENDPOINT: &str = "https://api.mailjet.com/v3.1/send";

#[derive(Debug, thiserror::Error)]
pub enum EmailError {
    #[error("email not configured")]
    NotConfigured,
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    #[error("mailjet status {status}: {body}")]
    HttpStatus {
        status: reqwest::StatusCode,
        body: String,
    },
}

#[allow(non_snake_case)]
#[derive(Debug, Clone, Serialize)]
struct MailjetPayload<'a> {
    Messages: Vec<MailjetMessage<'a>>,
}

#[allow(non_snake_case)]
#[derive(Debug, Clone, Serialize)]
struct MailjetMessage<'a> {
    From: MailjetAddress<'a>,
    To: Vec<MailjetAddress<'a>>,
    Subject: &'a str,
    TextPart: &'a str,
    HTMLPart: &'a str,
}

#[allow(non_snake_case)]
#[derive(Debug, Clone, Serialize)]
struct MailjetAddress<'a> {
    Email: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    Name: &'a str,
}

pub async fn send_mailjet(
    cfg: &config::MailjetSettings,
    to_email: &str,
    subject: &str,
    text: &str,
    html: &str,
) -> Result<(), EmailError> {
    if cfg.api_key.trim().is_empty()
        || cfg.api_secret.trim().is_empty()
        || cfg.from_email.trim().is_empty()
    {
        return Err(EmailError::NotConfigured);
    }

    let (url, fell_back) = resolve_endpoint(cfg);
    if fell_back {
        warn!(endpoint=%url, "mailjet: invalid custom base_url, falling back to default");
    }

    let from_name = cfg.from_name.as_str();
    let payload = MailjetPayload {
        Messages: vec![MailjetMessage {
            From: MailjetAddress {
                Email: cfg.from_email.as_str(),
                Name: from_name,
            },
            To: vec![MailjetAddress {
                Email: to_email,
                Name: "",
            }],
            Subject: subject,
            TextPart: text,
            HTMLPart: html,
        }],
    };

    let client = reqwest::Client::new();
    let resp = client
        .post(url)
        .basic_auth(cfg.api_key.trim(), Some(cfg.api_secret.trim()))
        .json(&payload)
        .send()
        .await?;

    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(EmailError::HttpStatus { status, body });
    }
    Ok(())
}

fn resolve_endpoint(cfg: &config::MailjetSettings) -> (String, bool) {
    let b = cfg.base_url.trim();
    if b.is_empty() {
        return (MAILJET_DEFAULT_ENDPOINT.to_string(), false);
    }
    // Add scheme if missing.
    let mut base = if b.starts_with("http://") || b.starts_with("https://") {
        b.to_string()
    } else {
        format!("https://{}", b)
    };
    // Strip trailing slash for consistency.
    while base.ends_with('/') {
        base.pop();
    }
    if base.ends_with("/v3.1/send") {
        return (base, false);
    }
    if base.contains("mailjet.com") {
        return (format!("{}/v3.1/send", base), false);
    }
    (MAILJET_DEFAULT_ENDPOINT.to_string(), true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_endpoint_defaults_and_normalizes() {
        let cfg = config::MailjetSettings::default();
        let (url, fell_back) = resolve_endpoint(&cfg);
        assert_eq!(url, MAILJET_DEFAULT_ENDPOINT);
        assert!(!fell_back);

        let cfg2 = config::MailjetSettings {
            base_url: "api.mailjet.com".to_string(),
            ..Default::default()
        };
        let (url2, fell_back2) = resolve_endpoint(&cfg2);
        assert_eq!(url2, "https://api.mailjet.com/v3.1/send");
        assert!(!fell_back2);

        let cfg3 = config::MailjetSettings {
            base_url: "https://api.mailjet.com/v3.1/send".to_string(),
            ..Default::default()
        };
        let (url3, fell_back3) = resolve_endpoint(&cfg3);
        assert_eq!(url3, "https://api.mailjet.com/v3.1/send");
        assert!(!fell_back3);

        let cfg4 = config::MailjetSettings {
            base_url: "https://example.com".to_string(),
            ..Default::default()
        };
        let (url4, fell_back4) = resolve_endpoint(&cfg4);
        assert_eq!(url4, MAILJET_DEFAULT_ENDPOINT);
        assert!(fell_back4);
    }

    #[tokio::test]
    async fn send_mailjet_requires_configuration() {
        let cfg = config::MailjetSettings::default();
        let err = send_mailjet(&cfg, "a@example.com", "subj", "text", "<p>html</p>")
            .await
            .unwrap_err();
        match err {
            EmailError::NotConfigured => {}
            _ => panic!("expected NotConfigured"),
        }
    }
}
