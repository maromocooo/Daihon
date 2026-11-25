use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{Context, Result};
use serde_json::{Map, Value, json};

const AGY_SKILL: &str =
    include_str!("../../integrations/agy/.agents/skills/daihon/SKILL.md");
const CODEX_SKILL: &str =
    include_str!("../../integrations/codex/.agents/skills/daihon/SKILL.md");
const CLAUDE_SKILL: &str =
    include_str!("../../integrations/claude/.claude/skills/daihon/SKILL.md");
const CLAUDE_CMD_REGISTER: &str =
    include_str!("../../integrations/claude/.claude/commands/daihon-register.md");
const CLAUDE_CMD_WATCH: &str =
    include_str!("../../integrations/claude/.claude/commands/daihon-watch.md");
const CLAUDE_CMD_STATUS: &str =
    include_str!("../../integrations/claude/.claude/commands/daihon-status.md");
const ALWAYS_ON_RULE: &str = include_str!("../../integrations/rules/scenario-first.md");

const RULE_START: &str = "<!-- daihon:start -->";
const RULE_END: &str = "<!-- daihon:end -->";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegrationTarget {
    Agy,
    Codex,
    Claude,
}

#[derive(Debug, Clone, Copy)]
pub struct InstallOptions {
    pub include_rules: bool,
    pub include_hooks: bool,
}

impl Default for InstallOptions {
    fn default() -> Self {
        Self {
            include_rules: true,
            include_hooks: true,
        }
    }
}

impl fmt::Display for IntegrationTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IntegrationTarget::Agy => f.write_str("agy"),
            IntegrationTarget::Codex => f.write_str("codex"),
            IntegrationTarget::Claude => f.write_str("claude"),
        }
    }
}

impl FromStr for IntegrationTarget {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "agy" => Ok(IntegrationTarget::Agy),
            "codex" => Ok(IntegrationTarget::Codex),
            "claude" => Ok(IntegrationTarget::Claude),
            other => Err(format!(
                "未対応の連携先 '{other}'（agy / codex / claude のいずれかを指定してください）"
            )),
        }
    }
}

/// Installs Skill, Always-On rule block, and lifecycle hooks into `target_project_dir`
/// while preserving any existing user rules or hook settings.
pub fn install_integration(
    target: IntegrationTarget,
    target_project_dir: &Path,
) -> Result<Vec<PathBuf>> {
    install_integration_with_options(target, target_project_dir, InstallOptions::default())
}

pub fn install_integration_with_options(
    target: IntegrationTarget,
    target_project_dir: &Path,
    options: InstallOptions,
) -> Result<Vec<PathBuf>> {
    let files: Vec<(&str, &str)> = match target {
        IntegrationTarget::Agy => vec![(".agents/skills/daihon/SKILL.md", AGY_SKILL)],
        IntegrationTarget::Codex => {
            vec![(".agents/skills/daihon/SKILL.md", CODEX_SKILL)]
        }
        IntegrationTarget::Claude => vec![
            (".claude/skills/daihon/SKILL.md", CLAUDE_SKILL),
            (".claude/commands/daihon-register.md", CLAUDE_CMD_REGISTER),
            (".claude/commands/daihon-watch.md", CLAUDE_CMD_WATCH),
            (".claude/commands/daihon-status.md", CLAUDE_CMD_STATUS),
        ],
    };

    let mut written = Vec::new();
    for (rel_path, content) in files {
        let dest = target_project_dir.join(rel_path);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&dest, content)
            .with_context(|| format!("連携ファイルの書き出しに失敗しました: {}", dest.display()))?;
        written.push(dest);
    }

    if options.include_rules {
        let rule_file = match target {
            IntegrationTarget::Agy | IntegrationTarget::Codex => {
                target_project_dir.join("AGENTS.md")
            }
            IntegrationTarget::Claude => target_project_dir.join("CLAUDE.md"),
        };
        upsert_managed_markdown_block(&rule_file, ALWAYS_ON_RULE)?;
        written.push(rule_file);
    }

    if options.include_hooks {
        match target {
            IntegrationTarget::Agy => {
                let hooks_path = target_project_dir.join(".agents/hooks.json");
                merge_agy_hooks_json(&hooks_path)?;
                written.push(hooks_path);
            }
            IntegrationTarget::Claude => {
                let settings_path = target_project_dir.join(".claude/settings.json");
                merge_claude_settings_json(&settings_path)?;
                written.push(settings_path);
            }
            IntegrationTarget::Codex => {}
        }
    }

    Ok(written)
}

/// Idempotently appends or updates the `<!-- daihon:start --> ... <!-- daihon:end -->`
/// managed block in an existing Markdown file without disturbing any other content.
pub fn upsert_managed_markdown_block(path: &Path, block: &str) -> Result<()> {
    let trimmed_block = block.trim();
    let updated = if path.exists() {
        let existing = fs::read_to_string(path)
            .with_context(|| format!("ルールファイルを読み込めません: {}", path.display()))?;
        if let (Some(start), Some(end)) = (existing.find(RULE_START), existing.find(RULE_END)) {
            let after_end = end + RULE_END.len();
            let before = &existing[..start];
            let after = existing[after_end..].trim_start_matches('\n');
            if after.is_empty() {
                format!("{before}{trimmed_block}\n")
            } else {
                format!("{before}{trimmed_block}\n\n{after}")
            }
        } else if existing.trim().is_empty() {
            format!("{trimmed_block}\n")
        } else if existing.ends_with('\n') {
            format!("{existing}\n{trimmed_block}\n")
        } else {
            format!("{existing}\n\n{trimmed_block}\n")
        }
    } else {
        format!("{trimmed_block}\n")
    };

    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, updated)
        .with_context(|| format!("ルールファイルを書き込めません: {}", path.display()))?;
    Ok(())
}

/// Safely merges the `daihon` hook entry into `.agents/hooks.json`.
pub fn merge_agy_hooks_json(path: &Path) -> Result<()> {
    let mut root = read_json_object_or_empty(path)?;
    root.insert(
        "daihon".to_string(),
        json!({
            "SessionStart": [
                {
                    "type": "command",
                    "command": "daihon hook-event agy-session-start",
                    "timeout": 10
                }
            ],
            "Stop": [
                {
                    "type": "command",
                    "command": "daihon hook-event agy-stop",
                    "timeout": 10
                }
            ]
        }),
    );

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, format!("{}\n", serde_json::to_string_pretty(&root)?))?;
    Ok(())
}

/// Safely merges Claude Code `SessionStart` and `Stop` hooks into `.claude/settings.json`.
pub fn merge_claude_settings_json(path: &Path) -> Result<()> {
    let mut root = read_json_object_or_empty(path)?;

    let hooks_val = root
        .entry("hooks".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if !hooks_val.is_object() {
        *hooks_val = Value::Object(Map::new());
    }
    let hooks_obj = hooks_val.as_object_mut().expect("hooks object");

    upsert_claude_hook_event(
        hooks_obj,
        "SessionStart",
        "daihon hook-event claude-session-start",
    );
    upsert_claude_hook_event(hooks_obj, "Stop", "daihon hook-event claude-stop");

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, format!("{}\n", serde_json::to_string_pretty(&root)?))?;
    Ok(())
}

fn upsert_claude_hook_event(hooks_obj: &mut Map<String, Value>, event_name: &str, command: &str) {
    let entry = hooks_obj
        .entry(event_name.to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    if !entry.is_array() {
        *entry = Value::Array(Vec::new());
    }
    let arr = entry.as_array_mut().expect("event array");

    // Remove any previous daihon managed entry for idempotency.
    arr.retain(|item| !contains_command(item, command));
    arr.push(json!({
        "hooks": [
            {
                "type": "command",
                "command": command
            }
        ]
    }));
}

fn contains_command(item: &Value, command: &str) -> bool {
    item.get("hooks")
        .and_then(Value::as_array)
        .is_some_and(|hooks| {
            hooks.iter().any(|h| {
                h.get("command")
                    .and_then(Value::as_str)
                    .is_some_and(|c| c == command)
            })
        })
}

fn read_json_object_or_empty(path: &Path) -> Result<Map<String, Value>> {
    if !path.exists() {
        return Ok(Map::new());
    }
    let content = fs::read_to_string(path)
        .with_context(|| format!("JSON 設定ファイルを読み込めません: {}", path.display()))?;
    if content.trim().is_empty() {
        return Ok(Map::new());
    }
    let parsed: Value = serde_json::from_str(&content)
        .with_context(|| format!("JSON 設定ファイルの構文エラー: {}", path.display()))?;
    parsed
        .as_object()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("{} のトップレベルが JSON オブジェクトではありません", path.display()))
}
