use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::config::{DEFAULT_CONFIG_FILE, HarnessConfig};
use crate::runner::INTERNAL_RUN_ENV;
use crate::watcher::drift::{compute_worktree_fingerprint, render_agent_context};

const HOOK_START: &str = "# >>> daihon post-commit >>>";
const HOOK_END: &str = "# <<< daihon post-commit <<<";
const DAEMON_PID_FILE: &str = "watch-daemon.pid";
const LAST_FP_FILE: &str = "last-watch-fp.txt";
const LAST_HOOK_TS_FILE: &str = "last-hook-trigger.txt";

/// Atomic lockfile preventing overlapping background watcher runs.
pub struct RunLock {
    path: PathBuf,
}

impl RunLock {
    pub fn try_acquire(state_dir: &Path) -> Result<Option<Self>> {
        fs::create_dir_all(state_dir)?;
        let path = state_dir.join("watch.lock");

        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                writeln!(file, "{}", std::process::id())?;
                Ok(Some(Self { path }))
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => Ok(None),
            Err(err) => Err(err).context("見張り用ロックファイルの作成に失敗しました"),
        }
    }
}

impl Drop for RunLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Parses human-friendly interval strings such as `30s`, `15m`, `1h`, or bare seconds.
pub fn parse_interval_secs(raw: &str) -> Result<u64> {
    let s = raw.trim().to_ascii_lowercase();
    if s.is_empty() {
        bail!("インターバル指定が空です");
    }

    let (num_str, multiplier) = if let Some(v) = s.strip_suffix('s') {
        (v, 1u64)
    } else if let Some(v) = s.strip_suffix('m') {
        (v, 60u64)
    } else if let Some(v) = s.strip_suffix('h') {
        (v, 3600u64)
    } else {
        (s.as_str(), 1u64)
    };

    let num: u64 = num_str
        .parse()
        .with_context(|| format!("インターバル '{raw}' を解釈できません（例: 30s, 15m, 1h）"))?;
    if num == 0 {
        bail!("インターバルは 1 秒以上を指定してください");
    }
    Ok(num.saturating_mul(multiplier))
}

/// Returns `true` if product code fingerprint matches the last completed watch run.
pub fn is_unchanged_since_last_watch(
    workdir: &Path,
    scenarios_dir: &Path,
    state_dir: &Path,
) -> bool {
    let fp_path = state_dir.join(LAST_FP_FILE);
    let Ok(prev) = fs::read_to_string(&fp_path) else {
        return false;
    };
    let prev = prev.trim();
    if prev.is_empty() {
        return false;
    }
    let current = compute_worktree_fingerprint(workdir, Some(scenarios_dir), Some(state_dir));
    current.fingerprint == prev
}

pub fn record_last_watch_fingerprint(state_dir: &Path, fingerprint: &str) -> Result<()> {
    fs::create_dir_all(state_dir)?;
    fs::write(state_dir.join(LAST_FP_FILE), format!("{fingerprint}\n"))?;
    Ok(())
}

pub fn is_within_hook_cooldown(state_dir: &Path, cooldown_secs: u64) -> bool {
    if cooldown_secs == 0 {
        return false;
    }
    let path = state_dir.join(LAST_HOOK_TS_FILE);
    let Ok(raw) = fs::read_to_string(&path) else {
        return false;
    };
    let Ok(prev_secs) = raw.trim().parse::<u64>() else {
        return false;
    };
    let now_secs = now_epoch_secs();
    now_secs.saturating_sub(prev_secs) < cooldown_secs
}

pub fn record_hook_trigger(state_dir: &Path) -> Result<()> {
    fs::create_dir_all(state_dir)?;
    fs::write(
        state_dir.join(LAST_HOOK_TS_FILE),
        format!("{}\n", now_epoch_secs()),
    )?;
    Ok(())
}

fn now_epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

/// Installs (or updates idempotently) a git `post-commit` hook that launches
/// `daihon watch --background --if-changed` after each commit without blocking git.
pub fn install_post_commit_hook(repo_root: &Path, scenario_bin: &str) -> Result<PathBuf> {
    let hooks_dir = repo_root.join(".git").join("hooks");
    if !hooks_dir.exists() {
        bail!(
            "`.git/hooks` が見つかりません（{} は Git リポジトリではありません）",
            repo_root.display()
        );
    }

    let hook_path = hooks_dir.join("post-commit");
    let managed_block = format!(
        "{HOOK_START}\nif command -v {scenario_bin} >/dev/null 2>&1; then\n  {scenario_bin} watch --background --if-changed >/dev/null 2>&1 || true\nfi\n{HOOK_END}\n"
    );

    let updated = if hook_path.exists() {
        let existing = fs::read_to_string(&hook_path)?;
        if let (Some(start), Some(end)) = (existing.find(HOOK_START), existing.find(HOOK_END)) {
            let after_end = end + HOOK_END.len();
            let rest = existing[after_end..].trim_start_matches('\n');
            format!("{}{}{}", &existing[..start], managed_block, rest)
        } else if existing.ends_with('\n') {
            format!("{existing}\n{managed_block}")
        } else {
            format!("{existing}\n\n{managed_block}")
        }
    } else {
        format!("#!/bin/sh\n\n{managed_block}")
    };

    fs::write(&hook_path, updated)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&hook_path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&hook_path, perms)?;
    }

    Ok(hook_path)
}

#[derive(Debug, Clone)]
pub struct BackgroundWatchRequest<'a> {
    pub current_exe: &'a Path,
    pub scenarios_dir: &'a Path,
    pub scenario_id: Option<&'a str>,
    pub state_dir: &'a Path,
    pub working_dir: &'a Path,
    pub dry_run_notify: bool,
    pub if_changed: bool,
    pub interval: Option<&'a str>,
}

/// Spawns `daihon watch` in a detached background process, logging output under `state_dir`.
/// When `interval` is provided, tracks the process PID in `watch-daemon.pid` for `--stop` / `--daemon-status`.
pub fn spawn_background_watch(req: &BackgroundWatchRequest<'_>) -> Result<u32> {
    fs::create_dir_all(req.state_dir)?;

    let is_daemon = req.interval.is_some();
    if is_daemon {
        let status = daemon_status(req.state_dir);
        if status.running
            && let Some(pid) = status.pid
        {
            bail!(
                "定期見張りデーモンはすでに稼働中です (pid: {pid})。停止するには `daihon watch --stop` を実行してください"
            );
        }
    }

    let log_name = if is_daemon {
        "watch-daemon.log"
    } else {
        "watch-background.log"
    };
    let log_path = req.state_dir.join(log_name);
    let stdout_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;
    let stderr_file = stdout_file.try_clone()?;

    let mut cmd = Command::new(req.current_exe);
    cmd.arg("--workdir")
        .arg(req.working_dir)
        .arg("watch")
        .arg("--dir")
        .arg(req.scenarios_dir)
        .current_dir(req.working_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file));

    if let Some(id) = req.scenario_id {
        cmd.arg("--scenario").arg(id);
    }
    if req.dry_run_notify {
        cmd.arg("--dry-run-notify");
    }
    if req.if_changed || is_daemon {
        cmd.arg("--if-changed");
    }
    if let Some(iv) = req.interval {
        cmd.arg("--interval").arg(iv);
    }

    let child = cmd
        .spawn()
        .context("バックグラウンド見張りプロセスの起動に失敗しました")?;
    let pid = child.id();

    if is_daemon {
        fs::write(req.state_dir.join(DAEMON_PID_FILE), format!("{pid}\n"))?;
    }

    Ok(pid)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonStatus {
    pub running: bool,
    pub pid: Option<u32>,
    pub log_path: PathBuf,
}

pub fn daemon_status(state_dir: &Path) -> DaemonStatus {
    let pid_path = state_dir.join(DAEMON_PID_FILE);
    let log_path = state_dir.join("watch-daemon.log");

    let Ok(raw) = fs::read_to_string(&pid_path) else {
        return DaemonStatus {
            running: false,
            pid: None,
            log_path,
        };
    };

    let Ok(pid) = raw.trim().parse::<u32>() else {
        let _ = fs::remove_file(&pid_path);
        return DaemonStatus {
            running: false,
            pid: None,
            log_path,
        };
    };

    if is_process_alive(pid) {
        DaemonStatus {
            running: true,
            pid: Some(pid),
            log_path,
        }
    } else {
        let _ = fs::remove_file(&pid_path);
        DaemonStatus {
            running: false,
            pid: None,
            log_path,
        }
    }
}

pub fn stop_daemon_watch(state_dir: &Path) -> Result<Option<u32>> {
    let status = daemon_status(state_dir);
    let Some(pid) = status.pid else {
        return Ok(None);
    };

    #[cfg(unix)]
    {
        let _ = Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .status();
    }

    let _ = fs::remove_file(state_dir.join(DAEMON_PID_FILE));
    Ok(Some(pid))
}

#[cfg(unix)]
fn is_process_alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

#[cfg(not(unix))]
fn is_process_alive(_pid: u32) -> bool {
    false
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookEventKind {
    AgySessionStart,
    AgyStop,
    ClaudeSessionStart,
    ClaudeStop,
}

impl fmt::Display for HookEventKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HookEventKind::AgySessionStart => f.write_str("agy-session-start"),
            HookEventKind::AgyStop => f.write_str("agy-stop"),
            HookEventKind::ClaudeSessionStart => f.write_str("claude-session-start"),
            HookEventKind::ClaudeStop => f.write_str("claude-stop"),
        }
    }
}

impl FromStr for HookEventKind {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "agy-session-start" => Ok(HookEventKind::AgySessionStart),
            "agy-stop" => Ok(HookEventKind::AgyStop),
            "claude-session-start" => Ok(HookEventKind::ClaudeSessionStart),
            "claude-stop" => Ok(HookEventKind::ClaudeStop),
            other => Err(format!(
                "未対応のフックイベント '{other}'（agy-session-start | agy-stop | claude-session-start | claude-stop）"
            )),
        }
    }
}

/// Resolves the target workspace root from hook stdin JSON (`cwd` or `workspacePaths[0]`),
/// handling the Antigravity hooks.json behavior where process CWD is `.agents/`.
pub fn resolve_hook_workdir(stdin_json: Option<&Value>, fallback_dir: &Path) -> PathBuf {
    if let Some(val) = stdin_json {
        if let Some(cwd) = val.get("cwd").and_then(Value::as_str)
            && !cwd.trim().is_empty()
        {
            return PathBuf::from(cwd);
        }
        if let Some(first) = val
            .get("workspacePaths")
            .and_then(Value::as_array)
            .and_then(|arr| arr.first())
            .and_then(Value::as_str)
            && !first.trim().is_empty()
        {
            return PathBuf::from(first);
        }
    }

    if let Some(name) = fallback_dir.file_name().and_then(|n| n.to_str())
        && matches!(name, ".agents" | ".claude")
        && let Some(parent) = fallback_dir.parent()
    {
        return parent.to_path_buf();
    }

    fallback_dir.to_path_buf()
}

/// Handles agent lifecycle hook events quickly (< 10ms) without blocking the coding agent.
/// Skips immediately when `DAIHON_INTERNAL_RUN=1` is set so child agents spawned by Daihon
/// (`register`, `togaki`, `watch`, `fix`) never recursively trigger background watches.
pub fn handle_hook_event(
    event: HookEventKind,
    stdin_raw: &str,
    fallback_dir: &Path,
    current_exe: &Path,
    spawn_watch: bool,
) -> Result<String> {
    if is_internal_child_run() {
        return Ok(match event {
            HookEventKind::AgySessionStart => "{}".to_string(),
            HookEventKind::AgyStop => r#"{"decision":"allow"}"#.to_string(),
            HookEventKind::ClaudeSessionStart | HookEventKind::ClaudeStop => String::new(),
        });
    }

    let stdin_json = serde_json::from_str::<Value>(stdin_raw.trim()).ok();
    let workdir = resolve_hook_workdir(stdin_json.as_ref(), fallback_dir);

    let cfg_path = workdir.join(DEFAULT_CONFIG_FILE);
    let config = HarnessConfig::load_from_dir(&workdir).unwrap_or_default();
    let scenarios_dir = config.resolve_scenarios_dir(&workdir, None);
    let state_dir = config.resolve_state_dir(&workdir);

    let initialized = cfg_path.exists() || scenarios_dir.exists();
    if !initialized {
        return Ok(match event {
            HookEventKind::AgySessionStart => "{}".to_string(),
            HookEventKind::AgyStop => r#"{"decision":"allow"}"#.to_string(),
            HookEventKind::ClaudeSessionStart | HookEventKind::ClaudeStop => String::new(),
        });
    }

    match event {
        HookEventKind::AgySessionStart => {
            let ctx = render_agent_context(&scenarios_dir, &state_dir, &config.drift)
                .unwrap_or_default();
            if ctx.trim().is_empty() {
                Ok("{}".to_string())
            } else {
                Ok(json!({
                    "injectSteps": [
                        {
                            "systemMessage": {
                                "systemMessage": ctx
                            }
                        }
                    ]
                })
                .to_string())
            }
        }
        HookEventKind::ClaudeSessionStart => {
            let ctx = render_agent_context(&scenarios_dir, &state_dir, &config.drift)
                .unwrap_or_default();
            Ok(ctx)
        }
        HookEventKind::AgyStop => {
            let continuation_active = stdin_json
                .as_ref()
                .and_then(|v| v.get("continuationActive"))
                .and_then(Value::as_bool)
                .unwrap_or(false);

            if !continuation_active {
                maybe_trigger_hook_watch(
                    &workdir,
                    &scenarios_dir,
                    &state_dir,
                    &config,
                    current_exe,
                    spawn_watch,
                );
            }
            // Always allow stop so the agent is never trapped in an infinite continuation loop.
            Ok(r#"{"decision":"allow"}"#.to_string())
        }
        HookEventKind::ClaudeStop => {
            maybe_trigger_hook_watch(
                &workdir,
                &scenarios_dir,
                &state_dir,
                &config,
                current_exe,
                spawn_watch,
            );
            Ok(String::new())
        }
    }
}

fn is_internal_child_run() -> bool {
    std::env::var(INTERNAL_RUN_ENV).ok().as_deref() == Some("1")
}

fn maybe_trigger_hook_watch(
    workdir: &Path,
    scenarios_dir: &Path,
    state_dir: &Path,
    config: &HarnessConfig,
    current_exe: &Path,
    spawn_watch: bool,
) {
    if is_within_hook_cooldown(state_dir, config.watcher.hook_cooldown_secs) {
        return;
    }
    if is_unchanged_since_last_watch(workdir, scenarios_dir, state_dir) {
        return;
    }
    let _ = record_hook_trigger(state_dir);
    if spawn_watch {
        let _ = spawn_background_watch(&BackgroundWatchRequest {
            current_exe,
            scenarios_dir,
            scenario_id: None,
            state_dir,
            working_dir: workdir,
            dry_run_notify: config.notify.dry_run,
            if_changed: true,
            interval: None,
        });
    }
}
