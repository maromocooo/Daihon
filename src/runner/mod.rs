pub mod agy;
pub mod claude;
pub mod codex;
pub mod mock;

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

use anyhow::{Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::HarnessConfig;
use crate::runner::agy::AgyRunner;
use crate::runner::claude::ClaudeRunner;
use crate::runner::codex::CodexRunner;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentKind {
    Agy,
    Codex,
    Claude,
    Mock,
}

impl AgentKind {
    pub fn default_binary(self) -> &'static str {
        match self {
            AgentKind::Agy => "agy",
            AgentKind::Codex => "codex",
            AgentKind::Claude => "claude",
            AgentKind::Mock => "mock",
        }
    }
}

impl fmt::Display for AgentKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AgentKind::Agy => f.write_str("agy"),
            AgentKind::Codex => f.write_str("codex"),
            AgentKind::Claude => f.write_str("claude"),
            AgentKind::Mock => f.write_str("mock"),
        }
    }
}

impl FromStr for AgentKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "agy" => Ok(AgentKind::Agy),
            "codex" => Ok(AgentKind::Codex),
            "claude" => Ok(AgentKind::Claude),
            "mock" => Ok(AgentKind::Mock),
            other => Err(format!(
                "不明なエージェント種別 '{other}'（agy / codex / claude のいずれかを指定してください）"
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionMode {
    ReadOnly,
    Verify,
    WorkspaceWrite,
}

#[derive(Debug, Clone)]
pub struct AgentRequest {
    pub working_dir: PathBuf,
    pub prompt: String,
    pub json_schema: Option<Value>,
    pub mode: ExecutionMode,
    /// Optional sandbox policy override (e.g. `read-only` / `workspace-write` / `danger-full-access`).
    pub sandbox_override: Option<String>,
    /// Extra environment variables injected into the agent process.
    pub env_vars: BTreeMap<String, String>,
    pub timeout_secs: u64,
}

#[derive(Debug, Clone)]
pub struct AgentResponse {
    pub agent_name: String,
    pub raw_stdout: String,
    pub raw_stderr: String,
    pub structured_json: Option<Value>,
}

pub trait AgentRunner {
    fn name(&self) -> &str;
    fn run(&self, req: &AgentRequest) -> Result<AgentResponse>;
}

pub fn build_runner(kind: AgentKind, config: &HarnessConfig) -> Result<Box<dyn AgentRunner>> {
    let binary = config.binary_for(kind);
    let extra_args = config.extra_args_for(kind);
    match kind {
        AgentKind::Agy => Ok(Box::new(
            AgyRunner::new(binary).with_extra_args(extra_args),
        )),
        AgentKind::Codex => Ok(Box::new(
            CodexRunner::new(binary).with_extra_args(extra_args),
        )),
        AgentKind::Claude => Ok(Box::new(
            ClaudeRunner::new(binary).with_extra_args(extra_args),
        )),
        AgentKind::Mock => bail!(
            "CLI から直接 `mock` ランナーは実行できません（自動テスト専用です。agy / codex / claude を指定してください）"
        ),
    }
}

/// Extracts a structured JSON object from raw agent CLI output, unwrapping common CLI envelopes
/// (`structured_output`, `result`, `response`, `text`) and stripping Markdown code fences if needed.
pub fn extract_json_value(raw: &str) -> Result<Value> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        bail!("エージェント出力が空でした");
    }

    if let Ok(val) = serde_json::from_str::<Value>(trimmed) {
        return unwrap_cli_envelope(val);
    }

    if let Some(fenced) = strip_json_fence(trimmed)
        && let Ok(val) = serde_json::from_str::<Value>(fenced)
    {
        return unwrap_cli_envelope(val);
    }

    if let (Some(start), Some(end)) = (trimmed.find('{'), trimmed.rfind('}'))
        && start < end
        && let Ok(val) = serde_json::from_str::<Value>(&trimmed[start..=end])
    {
        return unwrap_cli_envelope(val);
    }

    Err(anyhow!(
        "エージェント出力から JSON を抽出できませんでした:\n{}",
        trimmed.chars().take(400).collect::<String>()
    ))
}

fn unwrap_cli_envelope(val: Value) -> Result<Value> {
    unwrap_cli_envelope_depth(val, 0)
}

fn unwrap_cli_envelope_depth(val: Value, depth: usize) -> Result<Value> {
    if depth >= 4 {
        return Ok(val);
    }

    let Value::Object(ref map) = val else {
        return Ok(val);
    };

    for key in ["structured_output", "structuredOutput"] {
        if let Some(structured) = map.get(key)
            && !structured.is_null()
        {
            match structured {
                Value::Object(_) | Value::Array(_) => {
                    return unwrap_cli_envelope_depth(structured.clone(), depth + 1);
                }
                Value::String(inner) => {
                    if let Some(parsed) = parse_json_from_text(inner) {
                        return unwrap_cli_envelope_depth(parsed, depth + 1);
                    }
                }
                _ => {}
            }
        }
    }

    // Only unwrap generic envelope fields when the object looks like a CLI wrapper
    // rather than an already-unwrapped domain payload (`id` or `checks`).
    let looks_like_domain_payload = map.contains_key("id") || map.contains_key("checks");
    if !looks_like_domain_payload {
        for key in [
            "result",
            "response",
            "final_response",
            "output",
            "text",
            "content",
            "message",
            "data",
        ] {
            if let Some(inner_val) = map.get(key) {
                match inner_val {
                    Value::Object(_) => {
                        return unwrap_cli_envelope_depth(inner_val.clone(), depth + 1);
                    }
                    Value::String(inner) => {
                        if let Some(parsed) = parse_json_from_text(inner) {
                            return unwrap_cli_envelope_depth(parsed, depth + 1);
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    Ok(val)
}

fn parse_json_from_text(raw: &str) -> Option<Value> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(parsed) = serde_json::from_str::<Value>(trimmed) {
        return Some(parsed);
    }
    if let Some(fenced) = strip_json_fence(trimmed)
        && let Ok(parsed) = serde_json::from_str::<Value>(fenced)
    {
        return Some(parsed);
    }
    if let (Some(start), Some(end)) = (trimmed.find('{'), trimmed.rfind('}'))
        && start < end
        && let Ok(parsed) = serde_json::from_str::<Value>(&trimmed[start..=end])
    {
        return Some(parsed);
    }
    None
}

fn strip_json_fence(s: &str) -> Option<&str> {
    let start = s.find("```")?;
    let after_open = &s[start + 3..];
    let newline = after_open.find('\n')?;
    let content = &after_open[newline + 1..];
    let end = content.rfind("```")?;
    Some(content[..end].trim())
}
