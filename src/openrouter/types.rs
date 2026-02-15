use serde::{Deserialize, Serialize};

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
}
