use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use serde_json::json;

use crate::config::NotifyConfig;
use crate::model::scenario::{Priority, Scenario, ScenarioStatus};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationPayload {
    pub scenario_id: String,
    pub title: String,
    pub priority: Priority,
    pub status: ScenarioStatus,
    pub checked_at: String,
    pub message: String,
    pub topic: String,
}

pub trait Notifier {
    fn send(&self, payload: &NotificationPayload) -> Result<()>;
}

/// Decides whether a daihon status transition should trigger a smartphone notification.
///
/// Guarantees:
/// - Never notifies for partial steps (`in_progress`), failures (`failed`), or `pending`.
/// - Never sends duplicate notifications when `notified_pass_at` is already set.
/// - Notifies on transition into `passed` (all steps passed), OR into `ready_for_manual`
///   when `notify_on_ready_for_manual` is enabled (all auto steps passed across the scenario).
pub fn should_notify(
    previous: ScenarioStatus,
    next: ScenarioStatus,
    notified_pass_at: Option<&str>,
    notify_on_ready_for_manual: bool,
) -> bool {
    if notified_pass_at.is_some() {
        return false;
    }

    match next {
        ScenarioStatus::Passed => previous != ScenarioStatus::Passed,
        ScenarioStatus::ReadyForManual => {
            notify_on_ready_for_manual
                && previous != ScenarioStatus::ReadyForManual
                && previous != ScenarioStatus::Passed
        }
        ScenarioStatus::Pending | ScenarioStatus::InProgress | ScenarioStatus::Failed => false,
    }
}

pub fn build_pass_payload(
    scenario: &Scenario,
    topic: impl Into<String>,
    checked_at: impl Into<String>,
) -> NotificationPayload {
    let status = scenario.front_matter.status;
    let suffix = match status {
        ScenarioStatus::Passed => "全ステップを通過しました！",
        ScenarioStatus::ReadyForManual => {
            "全ステップの自動チェックを通過しました（実機確認・ドッグフーディング可能です）"
        }
        _ => "状態が更新されました",
    };

    let message = format!(
        "[{}] {} — {}",
        scenario.front_matter.priority, scenario.front_matter.title, suffix
    );

    NotificationPayload {
        scenario_id: scenario.front_matter.id.clone(),
        title: format!(
            "[{}] シナリオ通過: {}",
            scenario.front_matter.priority, scenario.front_matter.id
        ),
        priority: scenario.front_matter.priority,
        status,
        checked_at: checked_at.into(),
        message,
        topic: topic.into(),
    }
}

#[derive(Debug, Clone)]
pub struct NtfyNotifier {
    base_url: String,
}

impl NtfyNotifier {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
        }
    }
}

/// Normalizes a user-supplied ntfy topic so that `my-topic`, `ntfy.sh/my-topic`,
/// and `https://ntfy.sh/my-topic` all resolve cleanly to `my-topic`.
pub fn normalize_ntfy_topic(raw_topic: &str, base_url: &str) -> String {
    let mut t = raw_topic.trim().trim_matches('/').to_string();
    let base_trimmed = base_url.trim().trim_end_matches('/');

    if let Some(rest) = t.strip_prefix(&format!("{base_trimmed}/")) {
        t = rest.to_string();
    } else {
        let base_host = base_trimmed
            .strip_prefix("https://")
            .or_else(|| base_trimmed.strip_prefix("http://"))
            .unwrap_or(base_trimmed);

        for prefix in [
            format!("https://{base_host}/"),
            format!("http://{base_host}/"),
            format!("{base_host}/"),
        ] {
            if let Some(rest) = t.strip_prefix(&prefix) {
                t = rest.to_string();
                break;
            }
        }
    }

    t.trim_matches('/').to_string()
}

pub fn build_ntfy_json_body(payload: &NotificationPayload, base_url: &str) -> serde_json::Value {
    let topic = normalize_ntfy_topic(&payload.topic, base_url);
    let ntfy_priority: u8 = match payload.priority {
        Priority::P0 => 4, // high
        Priority::P1 => 3, // default
        Priority::P2 | Priority::P3 => 2, // low
    };

    json!({
        "topic": topic,
        "title": payload.title,
        "message": payload.message,
        "priority": ntfy_priority,
        "tags": ["white_check_mark"],
    })
}

impl Notifier for NtfyNotifier {
    fn send(&self, payload: &NotificationPayload) -> Result<()> {
        let topic = normalize_ntfy_topic(&payload.topic, &self.base_url);
        if topic.is_empty() {
            bail!(
                "ntfy のトピック名が設定されていません（環境変数 DAIHON_NTFY_TOPIC または .daihon.toml の notify.topic を設定してください）"
            );
        }

        // Use ntfy's JSON publish endpoint (POST <base_url>) so UTF-8 Japanese titles
        // and messages are sent in the JSON body rather than ASCII-only HTTP headers.
        let endpoint = self.base_url.trim_end_matches('/');
        let body = build_ntfy_json_body(payload, &self.base_url);
        let body_str = serde_json::to_string(&body)?;

        ureq::post(endpoint)
            .set("Content-Type", "application/json; charset=utf-8")
            .send_string(&body_str)
            .with_context(|| format!("ntfy 通知の送信に失敗しました ({endpoint}, topic: {topic})"))?;

        Ok(())
    }
}

/// Records notifications to a JSONL file without making network requests.
#[derive(Debug, Clone)]
pub struct DryRunNotifier {
    log_path: PathBuf,
}

impl DryRunNotifier {
    pub fn new(log_path: impl Into<PathBuf>) -> Self {
        Self {
            log_path: log_path.into(),
        }
    }

    pub fn log_path(&self) -> &Path {
        &self.log_path
    }

    pub fn read_records(&self) -> Result<Vec<NotificationPayload>> {
        if !self.log_path.exists() {
            return Ok(Vec::new());
        }
        let content = fs::read_to_string(&self.log_path)?;
        let mut records = Vec::new();
        for line in content.lines() {
            if !line.trim().is_empty() {
                records.push(serde_json::from_str(line)?);
            }
        }
        Ok(records)
    }
}

impl Notifier for DryRunNotifier {
    fn send(&self, payload: &NotificationPayload) -> Result<()> {
        if let Some(parent) = self.log_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log_path)?;
        let line = serde_json::to_string(payload)?;
        writeln!(file, "{line}")?;
        Ok(())
    }
}

#[derive(Debug, Default)]
pub struct NoopNotifier;

impl Notifier for NoopNotifier {
    fn send(&self, _payload: &NotificationPayload) -> Result<()> {
        Ok(())
    }
}

pub fn resolve_topic(config: &NotifyConfig) -> String {
    let raw = if let Some(ref topic) = config.topic
        && !topic.trim().is_empty()
    {
        topic.clone()
    } else {
        std::env::var(&config.topic_env).unwrap_or_default()
    };
    normalize_ntfy_topic(&raw, &config.ntfy_url)
}

pub fn build_notifier(
    config: &NotifyConfig,
    state_dir: &Path,
    force_dry_run: bool,
) -> (Box<dyn Notifier>, String) {
    let topic = resolve_topic(config);
    if force_dry_run || config.dry_run {
        let log_path = state_dir.join("notifications.jsonl");
        let resolved_topic = if topic.is_empty() {
            "dry-run-topic".to_string()
        } else {
            topic
        };
        (Box::new(DryRunNotifier::new(log_path)), resolved_topic)
    } else if topic.is_empty() {
        // If no topic is configured yet, fall back safely to recording locally rather than failing the watch run.
        let log_path = state_dir.join("notifications.jsonl");
        (
            Box::new(DryRunNotifier::new(log_path)),
            "unconfigured-topic".to_string(),
        )
    } else {
        (Box::new(NtfyNotifier::new(&config.ntfy_url)), topic)
    }
}
