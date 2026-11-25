use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::runner::AgentKind;

pub const DEFAULT_CONFIG_FILE: &str = ".daihon.toml";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HarnessConfig {
    pub scenarios_dir: PathBuf,
    pub state_dir: PathBuf,
    pub agents: AgentsConfig,
    pub watcher: WatcherConfig,
    pub drift: DriftConfig,
    pub environment: EnvironmentConfig,
    pub loop_policy: LoopPolicyConfig,
    pub notify: NotifyConfig,
}

impl Default for HarnessConfig {
    fn default() -> Self {
        Self {
            scenarios_dir: PathBuf::from("scenarios"),
            state_dir: PathBuf::from(".daihon"),
            agents: AgentsConfig::default(),
            watcher: WatcherConfig::default(),
            drift: DriftConfig::default(),
            environment: EnvironmentConfig::default(),
            loop_policy: LoopPolicyConfig::default(),
            notify: NotifyConfig::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentsConfig {
    pub register: AgentKind,
    pub developer: AgentKind,
    pub watcher: AgentKind,
    pub allow_same_agent: bool,
    /// Optional executable overrides (e.g. `{ claude = "/usr/local/bin/claude" }`).
    pub binaries: BTreeMap<String, String>,
    /// Optional extra CLI flags per agent (e.g. `{ claude = ["--model", "opus", "--effort", "high"] }`).
    pub extra_args: BTreeMap<String, Vec<String>>,
}

impl Default for AgentsConfig {
    fn default() -> Self {
        Self {
            register: AgentKind::Agy,
            developer: AgentKind::Agy,
            watcher: AgentKind::Codex,
            allow_same_agent: false,
            binaries: BTreeMap::new(),
            extra_args: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WatcherConfig {
    pub timeout_secs: u64,
    pub sandbox: String,
    /// Minimum seconds between hook-triggered background watcher runs.
    pub hook_cooldown_secs: u64,
}

impl Default for WatcherConfig {
    fn default() -> Self {
        Self {
            timeout_secs: 300,
            sandbox: "workspace-write".to_string(),
            hook_cooldown_secs: 60,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DriftConfig {
    /// Flag drift when the exact same step/check fails this many consecutive runs.
    pub stagnant_runs_threshold: usize,
    /// Flag drift when code changed this many consecutive runs without advancing the stopped check.
    pub code_changes_threshold: usize,
}

impl Default for DriftConfig {
    fn default() -> Self {
        Self {
            stagnant_runs_threshold: 3,
            code_changes_threshold: 2,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EnvironmentConfig {
    /// Free-form notes passed to the watcher agent (e.g. SSH host aliases, startup commands).
    pub notes: String,
    /// Target hosts involved in verification (e.g. `["mac-studio", "ubuntu-4090"]`).
    pub hosts: Vec<String>,
    /// Prerequisite commands or hints shown to the watcher before running checks.
    pub setup_commands: Vec<String>,
    /// Environment variables injected into watcher/developer agent processes.
    pub vars: BTreeMap<String, String>,
}

impl EnvironmentConfig {
    pub fn is_empty(&self) -> bool {
        self.notes.trim().is_empty()
            && self.hosts.is_empty()
            && self.setup_commands.is_empty()
            && self.vars.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LoopPolicyConfig {
    pub auto_start_dev: bool,
    pub max_retries: usize,
}

impl Default for LoopPolicyConfig {
    fn default() -> Self {
        Self {
            auto_start_dev: false,
            max_retries: 2,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NotifyConfig {
    pub provider: String,
    pub ntfy_url: String,
    pub topic: Option<String>,
    pub topic_env: String,
    pub dry_run: bool,
    pub notify_on_ready_for_manual: bool,
}

impl Default for NotifyConfig {
    fn default() -> Self {
        Self {
            provider: "ntfy".to_string(),
            ntfy_url: "https://ntfy.sh".to_string(),
            topic: None,
            topic_env: "DAIHON_NTFY_TOPIC".to_string(),
            dry_run: false,
            notify_on_ready_for_manual: true,
        }
    }
}

impl HarnessConfig {
    /// Loads `.daihon.toml` from `dir` if present, otherwise returns defaults.
    pub fn load_from_dir(dir: &Path) -> Result<Self> {
        let path = dir.join(DEFAULT_CONFIG_FILE);
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = fs::read_to_string(&path)
            .with_context(|| format!("設定ファイルを読み込めません: {}", path.display()))?;
        let config: Self = toml::from_str(&content)
            .with_context(|| format!("設定ファイルの構文エラー: {}", path.display()))?;
        Ok(config)
    }

    pub fn resolve_scenarios_dir(&self, base_dir: &Path, cli_override: Option<&Path>) -> PathBuf {
        match cli_override {
            Some(p) if p.is_absolute() => p.to_path_buf(),
            Some(p) => base_dir.join(p),
            None if self.scenarios_dir.is_absolute() => self.scenarios_dir.clone(),
            None => base_dir.join(&self.scenarios_dir),
        }
    }

    pub fn resolve_state_dir(&self, base_dir: &Path) -> PathBuf {
        if self.state_dir.is_absolute() {
            self.state_dir.clone()
        } else {
            base_dir.join(&self.state_dir)
        }
    }

    pub fn binary_for(&self, kind: AgentKind) -> String {
        self.agents
            .binaries
            .get(&kind.to_string())
            .cloned()
            .unwrap_or_else(|| kind.default_binary().to_string())
    }

    pub fn extra_args_for(&self, kind: AgentKind) -> Vec<String> {
        self.agents
            .extra_args
            .get(&kind.to_string())
            .cloned()
            .unwrap_or_default()
    }

    pub fn sample_toml() -> String {
        r#"# シナリオ駆動の開発ハーネス設定ファイル
scenarios_dir = "scenarios"
state_dir = ".daihon"

[agents]
# 登録・開発・見張りそれぞれに使うエージェント（agy | codex | claude）
# 既定では watcher != developer を強制します
register = "agy"
developer = "agy"
watcher = "codex"
allow_same_agent = false

[agents.extra_args]
# エージェント別の追加 CLI 引数（任意: モデルや推論強度の指定など）
# claude = ["--model", "opus", "--effort", "high"]
# codex = ["-m", "gpt-5-codex"]

[watcher]
timeout_secs = 300
# codex 見張り役のサンドボックス（read-only | workspace-write | danger-full-access）
# CLI 実行で一時状態を作るシナリオでは workspace-write、SSH 跨ぎ検証では danger-full-access を推奨
sandbox = "workspace-write"
# エージェントの Stop フック経由で見張りを起動する際の最小間隔（秒）
hook_cooldown_secs = 60

[drift]
# 同じステップ/チェックで何回連続停止したらドリフト警告を出すか
stagnant_runs_threshold = 3
# コード変更があるのに前進しない回数が何回連続したらドリフト警告を出すか
code_changes_threshold = 2

[environment]
# 複数マシン検証や環境前提を見張り役へ伝えるための設定（任意）
notes = ""
hosts = []
setup_commands = []

[environment.vars]
# 例: SAMPLE_CLI_HOME = "/tmp/sample-cli-state"

[loop_policy]
# true にすると確認なしで開発役へ自動着手します（既定は毎回ユーザー承認）
auto_start_dev = false
max_retries = 2

[notify]
provider = "ntfy"
ntfy_url = "https://ntfy.sh"
# トピック名は環境変数 DAIHON_NTFY_TOPIC での指定を推奨します
topic_env = "DAIHON_NTFY_TOPIC"
dry_run = false
# manual 確認項目を含むシナリオで、全 auto 項目通過時に通知するか
notify_on_ready_for_manual = true
"#
        .to_string()
    }
}
