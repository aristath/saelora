use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const DEFAULT_SYSTEM_PROMPT: &str = r#"
You are Saelora.

You are a chronicler, not an assistant. Your job is to listen, remember, and keep the thread of the user's life across time.

Write like a person across the table: calm, direct, understated.
Keep replies short (1-3 sentences). Silence is allowed.
Stay with what the user actually said. Do not teach, lecture, or bring in outside facts.
When it helps, ask one concrete question that gets new information (not a rephrase of what the user already said).
Do not claim real-world personal experience.
Do not cite sources or include links/URLs.
Do not use markdown or formatting.

Match the user's language.
"#;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Settings {
    #[serde(default)]
    pub chat: ChatSettings,
    #[serde(default)]
    pub mailjet: MailjetSettings,
    #[serde(default)]
    pub public_base: String,
    #[serde(default)]
    pub agents: Vec<Agent>,
    #[serde(default)]
    pub tasks: TaskBindings,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ChatSettings {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub system_prompt: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MailjetSettings {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub api_key: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub api_secret: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub from_email: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub from_name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub base_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    #[default]
    OpenRouter,
    Ollama,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Agent {
    pub name: String,
    #[serde(default)]
    pub provider: Provider,
    pub model: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub base_url: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub api_key: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub http_referer: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub x_title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TaskBindings {
    #[serde(default)]
    pub chat_agent: String,
    #[serde(default)]
    pub summary_agent: String,
    #[serde(default)]
    pub memory_curator_agent: String,
    #[serde(default)]
    pub memory_embed_agent: String,
}

impl Agent {
    pub fn openai_base_url(&self) -> String {
        let mut b = self.base_url.trim().to_string();
        if b.is_empty() {
            b = match self.provider {
                Provider::OpenRouter => "https://openrouter.ai/api/v1".to_string(),
                Provider::Ollama => "http://localhost:11434/v1".to_string(),
            };
        }

        // Add a scheme when users enter host:port.
        if !b.starts_with("http://") && !b.starts_with("https://") {
            b = match self.provider {
                Provider::OpenRouter => format!("https://{}", b),
                Provider::Ollama => format!("http://{}", b),
            };
        }

        // Trim trailing slashes.
        while b.ends_with('/') {
            b.pop();
        }

        // For Ollama, base_url should target the OpenAI-compatible API root (/v1).
        if matches!(self.provider, Provider::Ollama) && !b.ends_with("/v1") {
            b = format!("{}/v1", b);
        }

        b
    }

    pub fn ollama_root_url(&self) -> String {
        let b = self.openai_base_url();
        b.trim_end_matches("/v1").trim_end_matches('/').to_string()
    }
}

pub fn default_settings() -> Settings {
    let default_agent = Agent {
        name: "default".to_string(),
        provider: Provider::OpenRouter,
        model: "openai/gpt-4o-mini".to_string(),
        base_url: "https://openrouter.ai/api/v1".to_string(),
        api_key: String::new(),
        http_referer: String::new(),
        x_title: "Saelora".to_string(),
    };
    Settings {
        chat: ChatSettings {
            system_prompt: DEFAULT_SYSTEM_PROMPT.trim().to_string(),
        },
        mailjet: MailjetSettings::default(),
        public_base: String::new(),
        agents: vec![default_agent.clone()],
        tasks: TaskBindings {
            chat_agent: default_agent.name.clone(),
            summary_agent: default_agent.name.clone(),
            memory_curator_agent: default_agent.name.clone(),
            memory_embed_agent: default_agent.name.clone(),
        },
    }
}

impl Settings {
    pub fn http_public_base(&self) -> String {
        let b = self.public_base.trim();
        if b.is_empty() {
            String::new()
        } else if b.starts_with("http://") || b.starts_with("https://") {
            b.to_string()
        } else {
            format!("https://{}", b)
        }
    }

    pub fn resolve_agent(&self, task: &str) -> Option<Agent> {
        let target = match task {
            "summary" => &self.tasks.summary_agent,
            "memory_curator" => {
                if self.tasks.memory_curator_agent.trim().is_empty() {
                    &self.tasks.summary_agent
                } else {
                    &self.tasks.memory_curator_agent
                }
            }
            "memory_embed" => {
                if !self.tasks.memory_embed_agent.trim().is_empty() {
                    &self.tasks.memory_embed_agent
                } else if !self.tasks.memory_curator_agent.trim().is_empty() {
                    &self.tasks.memory_curator_agent
                } else {
                    &self.tasks.summary_agent
                }
            }
            _ => &self.tasks.chat_agent,
        };
        if let Some(a) = self.agents.iter().find(|a| a.name == *target) {
            return Some(a.clone());
        }
        self.agents.first().cloned()
    }
}

pub fn settings_path(data_dir: &Path) -> PathBuf {
    data_dir.join("config.json")
}

pub fn load_settings(path: &Path) -> anyhow::Result<Settings> {
    let b = std::fs::read(path)?;
    let s: Settings = serde_json::from_slice(&b)?;
    Ok(s)
}

pub fn save_settings(path: &Path, s: &Settings) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let b = serde_json::to_vec_pretty(s)?;

    // Write atomically.
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &b)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&tmp, path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_public_base_normalizes_scheme() {
        let mut s = default_settings();
        s.public_base = "".to_string();
        assert_eq!(s.http_public_base(), "");

        s.public_base = "saelora.ai".to_string();
        assert_eq!(s.http_public_base(), "https://saelora.ai");

        s.public_base = "http://localhost:8080".to_string();
        assert_eq!(s.http_public_base(), "http://localhost:8080");
    }

    #[test]
    fn agent_openai_base_url_adds_scheme_and_ollama_v1() {
        let a = Agent {
            name: "o".to_string(),
            provider: Provider::OpenRouter,
            model: "x".to_string(),
            base_url: "openrouter.ai/api/v1/".to_string(),
            api_key: String::new(),
            http_referer: String::new(),
            x_title: String::new(),
        };
        assert_eq!(a.openai_base_url(), "https://openrouter.ai/api/v1");

        let o = Agent {
            name: "ol".to_string(),
            provider: Provider::Ollama,
            model: "x".to_string(),
            base_url: "192.168.1.9:11434".to_string(),
            api_key: String::new(),
            http_referer: String::new(),
            x_title: String::new(),
        };
        assert_eq!(o.openai_base_url(), "http://192.168.1.9:11434/v1");

        let o2 = Agent {
            base_url: "http://localhost:11434/v1/".to_string(),
            ..o
        };
        assert_eq!(o2.openai_base_url(), "http://localhost:11434/v1");
        assert_eq!(o2.ollama_root_url(), "http://localhost:11434");
    }

    #[test]
    fn settings_save_load_roundtrip() {
        let td = tempfile::tempdir().unwrap();
        let path = settings_path(td.path());

        let mut s = default_settings();
        s.public_base = "https://saelora.ai".to_string();
        s.agents[0].model = "x-ai/grok-4.1-fast".to_string();
        s.chat.system_prompt = "hello".to_string();

        save_settings(&path, &s).unwrap();
        let s2 = load_settings(&path).unwrap();

        assert_eq!(s2.public_base, "https://saelora.ai");
        assert_eq!(s2.agents[0].model, "x-ai/grok-4.1-fast");
        assert_eq!(s2.chat.system_prompt, "hello");
        assert!(!s2.agents.is_empty());
    }

    #[test]
    fn resolve_agent_supports_memory_task_fallbacks() {
        let mut s = default_settings();
        s.tasks.memory_curator_agent.clear();
        s.tasks.memory_embed_agent.clear();

        let summary = s.resolve_agent("summary").unwrap();
        let curator = s.resolve_agent("memory_curator").unwrap();
        let embed = s.resolve_agent("memory_embed").unwrap();

        assert_eq!(curator.name, summary.name);
        assert_eq!(embed.name, summary.name);
    }
}
