use std::path::Path;

use tracing::info;

use crate::{config, openrouter};

#[derive(Debug, Clone)]
pub struct ResolvedTaskClient {
    pub settings: config::Settings,
    pub client: openrouter::Client,
    pub model: String,
}

pub fn load_settings_from_disk(data_dir: &Path) -> anyhow::Result<config::Settings> {
    let path = config::settings_path(data_dir);
    let mut cfg: config::Settings = match std::fs::read(&path) {
        Ok(b) => serde_json::from_slice(&b)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let s = config::default_settings();
            let _ = config::save_settings(&path, &s);
            s
        }
        Err(e) => return Err(anyhow::anyhow!(e)),
    };

    normalize_settings(&mut cfg);
    Ok(cfg)
}

pub fn normalize_settings(cfg: &mut config::Settings) {
    if cfg.agents.is_empty() {
        let s = config::default_settings();
        cfg.agents = s.agents;
    }
    if cfg.tasks.chat_agent.trim().is_empty() {
        cfg.tasks.chat_agent = cfg
            .agents
            .first()
            .map(|a| a.name.clone())
            .unwrap_or_default();
    }
    if cfg.tasks.summary_agent.trim().is_empty() {
        cfg.tasks.summary_agent = cfg
            .agents
            .first()
            .map(|a| a.name.clone())
            .unwrap_or_default();
    }
    if cfg.tasks.memory_agent.trim().is_empty() {
        cfg.tasks.memory_agent = cfg.tasks.summary_agent.clone();
    }
    if cfg.chat.system_prompt.trim().is_empty() {
        cfg.chat.system_prompt = config::DEFAULT_SYSTEM_PROMPT.trim().to_string();
    }
    for agent in &mut cfg.agents {
        if agent.normalized_temperature().is_none() {
            agent.temperature = None;
        }
    }

    if let Some(first) = cfg.agents.first() {
        let first_name = first.name.clone();
        for task in [
            &mut cfg.tasks.chat_agent,
            &mut cfg.tasks.summary_agent,
            &mut cfg.tasks.memory_agent,
        ] {
            if !cfg.agents.iter().any(|a| a.name == *task) {
                *task = first_name.clone();
            }
        }
    }
}

pub fn client_from_agent(agent: &config::Agent) -> anyhow::Result<openrouter::Client> {
    let base_url = agent.openai_base_url();
    openrouter::Client::new(openrouter::Config {
        api_key: agent.api_key.clone(),
        base_url,
        http_referer: if matches!(agent.provider, config::Provider::OpenRouter) {
            agent.http_referer.clone()
        } else {
            String::new()
        },
        x_title: if matches!(agent.provider, config::Provider::OpenRouter) {
            agent.x_title.clone()
        } else {
            String::new()
        },
    })
}

pub fn client_for_task_from_disk(
    data_dir: &Path,
    task: &str,
) -> anyhow::Result<ResolvedTaskClient> {
    let cfg = load_settings_from_disk(data_dir)?;
    let agent = cfg
        .resolve_agent(task)
        .ok_or_else(|| anyhow::anyhow!("no agents configured"))?;

    info!(
        task = task,
        agent = %agent.name,
        provider = ?agent.provider,
        model = %agent.model,
        base_url = %agent.openai_base_url(),
        "backend selected"
    );

    let client = client_from_agent(&agent)?;
    let model = if agent.model.trim().is_empty() {
        "openai/gpt-4o-mini".to_string()
    } else {
        agent.model.trim().to_string()
    };

    Ok(ResolvedTaskClient {
        settings: cfg,
        client,
        model,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_settings_populates_new_task_bindings() {
        let mut s = config::default_settings();
        s.tasks.memory_agent.clear();
        normalize_settings(&mut s);
        assert!(!s.tasks.memory_agent.is_empty());
        assert_eq!(s.tasks.memory_agent, s.tasks.summary_agent);
    }
}
