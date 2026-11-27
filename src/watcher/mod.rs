pub mod drift;
pub mod trigger;

use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use chrono::Local;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::config::{DriftConfig, EnvironmentConfig, HarnessConfig};
use crate::model::scenario::{CheckState, Scenario, ScenarioStatus, Step, VerifyMode};
use crate::model::store::save_updated_scenario;
use crate::model::validator::validate_file;
use crate::notify::{Notifier, build_pass_payload, should_notify};
use crate::runner::{AgentKind, AgentRequest, AgentRunner, ExecutionMode};
use crate::watcher::drift::{
    DriftSummary, analyze_drift_from_reports, clear_watch_abort, compute_worktree_fingerprint,
    load_runs_for_scenario, record_watch_abort, sync_next_task_file,
};

#[derive(Debug, Clone)]
pub struct ParentContext {
    pub id: String,
    pub title: String,
    pub overview: String,
    pub hosts: Vec<String>,
    pub step_titles: Vec<(usize, String)>,
}

impl ParentContext {
    pub fn from_scenario(sc: &Scenario) -> Self {
        Self {
            id: sc.front_matter.id.clone(),
            title: sc.front_matter.title.clone(),
            overview: sc.overview.clone(),
            hosts: sc.front_matter.hosts.clone(),
            step_titles: sc
                .steps
                .iter()
                .map(|s| (s.number, s.title.clone()))
                .collect(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct WatchOptions {
    pub working_dir: PathBuf,
    pub state_dir: PathBuf,
    pub timeout_secs: u64,
    pub sandbox: Option<String>,
    pub environment: EnvironmentConfig,
    pub drift: DriftConfig,
    pub parent_context: Option<ParentContext>,
    pub notify_topic: String,
    pub notify_on_ready_for_manual: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckEvalRecord {
    pub check_id: String,
    pub mode: VerifyMode,
    pub state: CheckState,
    pub command_run: Option<String>,
    pub evidence: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepEvalRecord {
    pub step_number: usize,
    pub step_title: String,
    pub checks: Vec<CheckEvalRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchRunReport {
    pub run_id: String,
    pub scenario_id: String,
    pub scenario_path: PathBuf,
    pub watcher_agent: String,
    pub started_at: String,
    pub finished_at: String,
    pub previous_status: ScenarioStatus,
    pub new_status: ScenarioStatus,
    pub stopped_step: Option<usize>,
    pub stopped_check: Option<String>,
    pub stopped_reason: Option<String>,
    pub step_results: Vec<StepEvalRecord>,
    pub notified: bool,
    #[serde(default)]
    pub git_head: Option<String>,
    #[serde(default)]
    pub worktree_fingerprint: Option<String>,
    #[serde(default)]
    pub drift: Option<DriftSummary>,
    #[serde(default)]
    pub report_path: PathBuf,
}

#[derive(Debug, Deserialize)]
struct StepAgentResponse {
    checks: Vec<AgentCheckOutcome>,
}

#[derive(Debug, Deserialize)]
struct AgentCheckOutcome {
    check_id: String,
    passed: bool,
    #[serde(default)]
    command_run: Option<String>,
    #[serde(default)]
    evidence: Option<String>,
    #[serde(default)]
    reason: Option<String>,
}

/// Enforces the core rule that the watcher agent must be distinct from the developer agent,
/// unless explicitly opted out via config or CLI flag.
pub fn resolve_distinct_watcher_agent(
    config: &HarnessConfig,
    override_watcher: Option<AgentKind>,
    allow_same_agent: bool,
) -> Result<AgentKind> {
    let watcher = override_watcher.unwrap_or(config.agents.watcher);
    let dev = config.agents.developer;
    if watcher == dev && !(allow_same_agent || config.agents.allow_same_agent) {
        bail!(
            "見張り役 ({watcher}) は開発役 ({dev}) と別のエージェントを指定してください。\n\
             .daihon.toml の agents.watcher を変更するか、明示的に --allow-same-agent を指定してください。"
        );
    }
    Ok(watcher)
}

/// Resolves the ordered list of watcher agent candidates: primary watcher followed by any
/// configured `watcher_fallbacks` that stay distinct from `developer` (unless allowed).
pub fn resolve_watcher_candidates(
    config: &HarnessConfig,
    override_watcher: Option<AgentKind>,
    allow_same_agent: bool,
) -> Result<Vec<AgentKind>> {
    let primary = resolve_distinct_watcher_agent(config, override_watcher, allow_same_agent)?;
    let dev = config.agents.developer;
    let allow_same = allow_same_agent || config.agents.allow_same_agent;

    let mut candidates = vec![primary];
    for &fb in &config.agents.watcher_fallbacks {
        if fb == AgentKind::Mock || candidates.contains(&fb) {
            continue;
        }
        if fb == dev && !allow_same {
            continue;
        }
        candidates.push(fb);
    }
    Ok(candidates)
}

/// Returns true when a watcher execution error indicates quota/rate-limit, timeout, or runner
/// unavailability where retrying with a fallback distinct watcher agent is appropriate.
pub fn is_fallback_worthy_error(err: &anyhow::Error) -> bool {
    let msg = format!("{err:#}").to_ascii_lowercase();
    [
        "rate limit",
        "usage limit",
        "quota",
        "429",
        "too many requests",
        "deadline_exceeded",
        "print_timeout",
        "timed out",
        "timeout",
        "overloaded",
        "capacity",
        "の起動に失敗",
        "not found",
        "使用上限",
    ]
    .iter()
    .any(|needle| msg.contains(needle))
}

/// Evaluates a scenario trying `watcher_runners` in priority order, automatically failing over
/// to the next distinct watcher on rate-limit/quota/timeout errors, and recording an abort
/// marker if all watchers fail so `status` and `tui` never silently display stale results.
pub fn watch_scenario_with_fallbacks(
    scenario_path: &Path,
    watcher_runners: &[&dyn AgentRunner],
    notifier: &dyn Notifier,
    options: &WatchOptions,
) -> Result<WatchRunReport> {
    if watcher_runners.is_empty() {
        bail!("見張り役エージェントが 1 つも指定されていません");
    }

    let scenario_id = validate_file(scenario_path)
        .map(|s| s.front_matter.id)
        .unwrap_or_else(|_| {
            scenario_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string()
        });

    let mut last_err: Option<(String, anyhow::Error)> = None;
    for (idx, runner) in watcher_runners.iter().enumerate() {
        if idx > 0 {
            println!(
                "  -> フォールバック見張り役 ({}) へ自動切替して再試行します...",
                runner.name()
            );
        }
        match watch_scenario(scenario_path, *runner, notifier, options) {
            Ok(report) => {
                clear_watch_abort(&options.state_dir, &report.scenario_id);
                return Ok(report);
            }
            Err(err) => {
                let fallback_worthy = is_fallback_worthy_error(&err);
                let has_next = idx + 1 < watcher_runners.len();
                if fallback_worthy && has_next {
                    eprintln!(
                        "  ⚠ 見張り役 ({}) が中断しました（次候補へフォールバック）: {}",
                        runner.name(),
                        err.to_string().lines().next().unwrap_or("")
                    );
                    last_err = Some((runner.name().to_string(), err));
                    continue;
                }
                let _ = record_watch_abort(
                    &options.state_dir,
                    &scenario_id,
                    runner.name(),
                    fallback_worthy,
                    &format!("{err:#}"),
                );
                return Err(err);
            }
        }
    }

    let (agent, err) = last_err.expect("at least one runner attempted");
    let _ = record_watch_abort(
        &options.state_dir,
        &scenario_id,
        &agent,
        true,
        &format!("{err:#}"),
    );
    Err(err)
}

pub fn step_eval_json_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["checks"],
        "properties": {
            "checks": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["check_id", "passed", "command_run", "evidence", "reason"],
                    "properties": {
                        "check_id": { "type": "string" },
                        "passed": { "type": "boolean" },
                        "command_run": { "type": ["string", "null"] },
                        "evidence": { "type": ["string", "null"] },
                        "reason": { "type": ["string", "null"] }
                    }
                }
            }
        }
    })
}

/// Evaluates a single scenario step-by-step using the watcher agent, writes back updated
/// statuses to the scenario Markdown file, persists run evidence, and notifies only when
/// transitioning to all-passed (or all-auto-passed when configured).
pub fn watch_scenario(
    scenario_path: &Path,
    watcher_runner: &dyn AgentRunner,
    notifier: &dyn Notifier,
    options: &WatchOptions,
) -> Result<WatchRunReport> {
    let mut scenario = validate_file(scenario_path).map_err(|issues| {
        let details = issues
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n  ");
        anyhow!(
            "見張り開始前のシナリオ形式チェックに失敗しました ({}):\n  {details}",
            scenario_path.display()
        )
    })?;

    let started_at = Local::now().to_rfc3339();
    let run_id = Local::now().format("%Y%m%d-%H%M%S-%3f").to_string();
    let previous_status = scenario.front_matter.status;
    let prev_passing_cmds =
        collect_previous_passing_commands(&options.state_dir, &scenario.front_matter.id);

    let mut step_results = Vec::new();
    let mut stopped_step = None;
    let mut stopped_check = None;
    let mut stopped_reason = None;
    let mut stopped_at_index: Option<usize> = None;

    let total_steps = scenario.steps.len();
    for idx in 0..total_steps {
        let step_snapshot = scenario.steps[idx].clone();
        let auto_checks: Vec<_> = step_snapshot
            .checks
            .iter()
            .filter(|c| c.mode == VerifyMode::Auto)
            .collect();

        if auto_checks.is_empty() {
            println!(
                "  Step {}/{}: {} -> SKIP (manual 項目のみ)",
                step_snapshot.number, total_steps, step_snapshot.title
            );
            let records = step_snapshot
                .checks
                .iter()
                .map(|c| CheckEvalRecord {
                    check_id: c.id.clone(),
                    mode: c.mode,
                    state: c.state,
                    command_run: None,
                    evidence: Some("manual 項目のため見張り役はスキップ".to_string()),
                    reason: c.reason.clone(),
                })
                .collect();
            step_results.push(StepEvalRecord {
                step_number: step_snapshot.number,
                step_title: step_snapshot.title.clone(),
                checks: records,
            });
            continue;
        }

        print!(
            "  Step {}/{}: {} ... ",
            step_snapshot.number, total_steps, step_snapshot.title
        );
        let _ = io::stdout().flush();

        let prompt = build_watcher_step_prompt(
            &scenario,
            &step_snapshot,
            &options.environment,
            options.parent_context.as_ref(),
            &prev_passing_cmds,
        );
        let response = watcher_runner.run(&AgentRequest {
            working_dir: options.working_dir.clone(),
            prompt,
            json_schema: Some(step_eval_json_schema()),
            mode: ExecutionMode::Verify,
            sandbox_override: options.sandbox.clone(),
            env_vars: options.environment.vars.clone(),
            timeout_secs: options.timeout_secs,
        })?;

        let json_val = response
            .structured_json
            .ok_or_else(|| anyhow!("見張り役から構造化 JSON が返りませんでした"))?;
        let preview: String = json_val.to_string().chars().take(300).collect();
        let parsed: StepAgentResponse = serde_json::from_value(json_val).with_context(|| {
            format!("見張り役の JSON 応答が仕様と一致しません（受信内容: {preview}）")
        })?;

        let mut outcome_map: HashMap<String, AgentCheckOutcome> = HashMap::new();
        for item in parsed.checks {
            outcome_map.insert(item.check_id.clone(), item);
        }

        let mut check_records = Vec::new();
        let mut step_failed = false;

        for check in &mut scenario.steps[idx].checks {
            if check.mode == VerifyMode::Manual {
                check_records.push(CheckEvalRecord {
                    check_id: check.id.clone(),
                    mode: check.mode,
                    state: check.state,
                    command_run: None,
                    evidence: Some("manual 項目のため状態保持".to_string()),
                    reason: check.reason.clone(),
                });
                continue;
            }

            if let Some(outcome) = outcome_map.remove(&check.id) {
                if outcome.passed {
                    check.state = CheckState::Passed;
                    check.reason = None;
                } else {
                    check.state = CheckState::Failed;
                    let reason_text = outcome
                        .reason
                        .clone()
                        .filter(|s| !s.trim().is_empty())
                        .unwrap_or_else(|| "検証条件を満たしませんでした".to_string());
                    check.reason = Some(reason_text.clone());
                    if !step_failed {
                        step_failed = true;
                        stopped_step = Some(step_snapshot.number);
                        stopped_check = Some(check.id.clone());
                        stopped_reason = Some(reason_text);
                    }
                }
                check_records.push(CheckEvalRecord {
                    check_id: check.id.clone(),
                    mode: check.mode,
                    state: check.state,
                    command_run: outcome.command_run,
                    evidence: outcome.evidence,
                    reason: check.reason.clone(),
                });
            } else {
                check.state = CheckState::Failed;
                let msg = "見張り役がこのチェック項目の検証結果を返しませんでした".to_string();
                check.reason = Some(msg.clone());
                if !step_failed {
                    step_failed = true;
                    stopped_step = Some(step_snapshot.number);
                    stopped_check = Some(check.id.clone());
                    stopped_reason = Some(msg.clone());
                }
                check_records.push(CheckEvalRecord {
                    check_id: check.id.clone(),
                    mode: check.mode,
                    state: check.state,
                    command_run: None,
                    evidence: None,
                    reason: Some(msg),
                });
            }
        }

        step_results.push(StepEvalRecord {
            step_number: step_snapshot.number,
            step_title: step_snapshot.title.clone(),
            checks: check_records,
        });

        if step_failed {
            println!(
                "FAIL ({})",
                stopped_check.as_deref().unwrap_or("-")
            );
            stopped_at_index = Some(idx);
            break;
        } else {
            println!("OK");
        }
    }

    // Reset auto checks on steps after the stopped step so the stopped point stays unambiguous.
    if let Some(stop_idx) = stopped_at_index {
        for later_step in scenario.steps.iter_mut().skip(stop_idx + 1) {
            for check in &mut later_step.checks {
                if check.mode == VerifyMode::Auto {
                    check.state = CheckState::Pending;
                    check.reason = None;
                }
            }
        }
    }

    let finished_at = Local::now().to_rfc3339();
    scenario.recompute_statuses();
    let new_status = scenario.front_matter.status;
    scenario.front_matter.last_checked_at = Some(finished_at.clone());
    scenario.front_matter.last_checked_by = Some(watcher_runner.name().to_string());

    // Clear notified timestamp on regression so a future recovery notifies again.
    if matches!(
        new_status,
        ScenarioStatus::Failed | ScenarioStatus::Pending | ScenarioStatus::InProgress
    ) {
        scenario.front_matter.notified_pass_at = None;
    }

    let mut notified = false;
    if should_notify(
        previous_status,
        new_status,
        scenario.front_matter.notified_pass_at.as_deref(),
        options.notify_on_ready_for_manual,
    ) {
        let payload = build_pass_payload(&scenario, &options.notify_topic, &finished_at);
        notifier.send(&payload)?;
        scenario.front_matter.notified_pass_at = Some(finished_at.clone());
        notified = true;
    }

    save_updated_scenario(scenario_path, &scenario)?;

    let snapshot = compute_worktree_fingerprint(
        &options.working_dir,
        scenario_path.parent(),
        Some(&options.state_dir),
    );

    let mut report = WatchRunReport {
        run_id: run_id.clone(),
        scenario_id: scenario.front_matter.id.clone(),
        scenario_path: scenario_path.to_path_buf(),
        watcher_agent: watcher_runner.name().to_string(),
        started_at,
        finished_at,
        previous_status,
        new_status,
        stopped_step,
        stopped_check,
        stopped_reason,
        step_results,
        notified,
        git_head: snapshot.git_head,
        worktree_fingerprint: Some(snapshot.fingerprint),
        drift: None,
        report_path: PathBuf::new(),
    };

    let mut history = load_runs_for_scenario(&options.state_dir, &report.scenario_id)?;
    history.push(report.clone());
    report.drift = analyze_drift_from_reports(&history, &options.drift);

    let saved_report_path = persist_run_report(&options.state_dir, &report)?;
    report.report_path = saved_report_path;
    clear_watch_abort(&options.state_dir, &report.scenario_id);

    if let Some(scenarios_dir) = scenario_path.parent() {
        let _ = sync_next_task_file(scenarios_dir, &options.state_dir, &options.drift);
    }

    Ok(report)
}

/// Updates a single check item manually (primarily for `[manual]` verification items)
/// and triggers notification if this completes the scenario.
pub fn update_check_manually(
    scenario_path: &Path,
    check_id: &str,
    new_state: CheckState,
    reason: Option<String>,
    notifier: &dyn Notifier,
    notify_topic: &str,
    notify_on_ready_for_manual: bool,
) -> Result<(Scenario, bool)> {
    let mut scenario = validate_file(scenario_path).map_err(|issues| {
        let details = issues
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n  ");
        anyhow!("シナリオ形式エラー:\n  {details}")
    })?;

    let prev_status = scenario.front_matter.status;
    let mut found = false;
    for step in &mut scenario.steps {
        for check in &mut step.checks {
            if check.id == check_id {
                check.state = new_state;
                check.reason = match new_state {
                    CheckState::Failed => Some(
                        reason
                            .clone()
                            .unwrap_or_else(|| "手動確認で NG となりました".to_string()),
                    ),
                    CheckState::Passed | CheckState::Pending => None,
                };
                found = true;
            }
        }
    }

    if !found {
        bail!(
            "チェック項目 ID `{check_id}` がシナリオ `{}` 内に見つかりません",
            scenario.front_matter.id
        );
    }

    let now = Local::now().to_rfc3339();
    scenario.recompute_statuses();
    let new_status = scenario.front_matter.status;
    scenario.front_matter.last_checked_at = Some(now.clone());
    scenario.front_matter.last_checked_by = Some("manual".to_string());

    if matches!(
        new_status,
        ScenarioStatus::Failed | ScenarioStatus::Pending | ScenarioStatus::InProgress
    ) {
        scenario.front_matter.notified_pass_at = None;
    }

    let mut notified = false;
    if should_notify(
        prev_status,
        new_status,
        scenario.front_matter.notified_pass_at.as_deref(),
        notify_on_ready_for_manual,
    ) {
        let payload = build_pass_payload(&scenario, notify_topic, &now);
        notifier.send(&payload)?;
        scenario.front_matter.notified_pass_at = Some(now);
        notified = true;
    }

    save_updated_scenario(scenario_path, &scenario)?;
    Ok((scenario, notified))
}

pub fn load_latest_run_for_scenario(
    state_dir: &Path,
    scenario_id: &str,
) -> Result<Option<WatchRunReport>> {
    let runs_dir = state_dir.join("runs");
    if !runs_dir.exists() {
        return Ok(None);
    }

    let suffix = format!("-{scenario_id}.json");
    let mut candidates: Vec<PathBuf> = fs::read_dir(&runs_dir)?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|name| name.ends_with(&suffix))
        })
        .collect();
    candidates.sort();

    let Some(latest) = candidates.pop() else {
        return Ok(None);
    };
    let content = fs::read_to_string(&latest)?;
    let report: WatchRunReport = serde_json::from_str(&content)?;
    Ok(Some(report))
}

fn persist_run_report(state_dir: &Path, report: &WatchRunReport) -> Result<PathBuf> {
    let runs_dir = state_dir.join("runs");
    fs::create_dir_all(&runs_dir)?;
    let path = runs_dir.join(format!("{}-{}.json", report.run_id, report.scenario_id));
    fs::write(&path, serde_json::to_string_pretty(report)?)?;
    Ok(path)
}

/// Collects the most recent passing `command_run` per check ID from historical watch reports
/// so subsequent watch runs reuse proven, deterministic verification commands.
fn collect_previous_passing_commands(
    state_dir: &Path,
    scenario_id: &str,
) -> HashMap<String, String> {
    let mut cmds = HashMap::new();
    let Ok(history) = load_runs_for_scenario(state_dir, scenario_id) else {
        return cmds;
    };
    for rep in history.iter().rev() {
        for step in &rep.step_results {
            for chk in &step.checks {
                if chk.state == CheckState::Passed
                    && !cmds.contains_key(&chk.check_id)
                    && let Some(ref cmd) = chk.command_run
                    && !cmd.trim().is_empty()
                {
                    cmds.insert(chk.check_id.clone(), cmd.trim().to_string());
                }
            }
        }
    }
    cmds
}

fn build_watcher_step_prompt(
    scenario: &Scenario,
    step: &Step,
    env_cfg: &EnvironmentConfig,
    parent_ctx: Option<&ParentContext>,
    prev_passing_cmds: &HashMap<String, String>,
) -> String {
    let auto_list = step
        .checks
        .iter()
        .filter(|c| c.mode == VerifyMode::Auto)
        .map(|c| {
            if let Some(prev_cmd) = prev_passing_cmds.get(&c.id) {
                format!(
                    "- `{}`: {}\n  - 前回通過時の検証コマンド（参考・再現性向上用）: `{prev_cmd}`",
                    c.id, c.description
                )
            } else {
                format!("- `{}`: {}", c.id, c.description)
            }
        })
        .collect::<Vec<_>>()
        .join("\n");

    let mut all_hosts: Vec<String> = env_cfg.hosts.clone();
    if let Some(p) = parent_ctx {
        for h in &p.hosts {
            if !all_hosts.contains(h) {
                all_hosts.push(h.clone());
            }
        }
    }
    for h in &scenario.front_matter.hosts {
        if !all_hosts.contains(h) {
            all_hosts.push(h.clone());
        }
    }

    let mut env_lines = Vec::new();
    if !all_hosts.is_empty() {
        env_lines.push(format!("- 対象ホスト: {}", all_hosts.join(", ")));
    }
    if !env_cfg.setup_commands.is_empty() {
        env_lines.push(format!(
            "- 前提コマンド: {}",
            env_cfg.setup_commands.join(" ; ")
        ));
    }
    if !env_cfg.notes.trim().is_empty() {
        env_lines.push(format!("- 環境メモ:\n{}", env_cfg.notes.trim()));
    }
    let env_section = if env_lines.is_empty() {
        String::new()
    } else {
        format!("\n## 検証環境の前提\n{}\n", env_lines.join("\n"))
    };

    let parent_section = match parent_ctx {
        Some(p) => {
            let p_steps = p
                .step_titles
                .iter()
                .map(|(n, t)| format!("  - Step {n}: {t}"))
                .collect::<Vec<_>>()
                .join("\n");
            format!(
                "\n## 親シナリオ（前提として既に通過済みの本筋）\n- ID: {}\n- タイトル: {}\n- 概要:\n{}\n- 親ステップ:\n{}\n",
                p.id, p.title, p.overview, p_steps
            )
        }
        None => String::new(),
    };

    let role_target = if scenario.is_togaki() {
        "ト書き（親シナリオに追加された枝葉・品質要件）"
    } else {
        "シナリオ"
    };

    format!(
        r#"あなたはシナリオ駆動の開発ハーネス「Daihon」の「見張り役（独立した自動検証エージェント）」です。
対象プロダクトのソースコードは一切変更せず、以下の{role_target}のステップに含まれる各 `[auto]` チェック項目を実際にコマンド実行等で検証し、
事実に基づいた結果を JSON Schema 通りに返してください。

厳守事項:
1. プロダクトのソースコード・設定ファイル・シナリオ Markdown ファイル自体は絶対に編集しないこと。
2. 検証対象コマンドの実行に伴う一時的なランタイム状態（例: セッション状態ファイルや一時出力）の作成は許可される。前のステップで作られた状態があればそのまま利用して検証すること。
3. 推測で passed: true にしないこと。実際にコマンドを実行して確認した根拠を `command_run` と `evidence` に書くこと。前回通過時の検証コマンドが提示されている場合は、同じ手順を優先して再現性高く検証すること。
4. 失敗した項目には、何が足りずどう失敗したかを `reason` に具体的に書くこと。
5. サンドボックス環境内では macOS ネイティブ GUI (.app) 等の直接起動が OS 制約（LaunchServices / Seatbelt 等）で失敗する場合がある。その場合は「検証環境の前提」の環境メモに従い、ビルド・ヘッドレス起動・自動テストで `[auto]` 項目を検証すること（実機 GUI の見た目・操作感は `[manual]` の責務）。

## 検証対象の{role_target}
- ID: {}
- タイトル: {}
- 概要:
{}
{}{}
## 今回の検証対象: Step {}: {}
{}
"#,
        scenario.front_matter.id,
        scenario.front_matter.title,
        scenario.overview,
        parent_section,
        env_section,
        step.number,
        step.title,
        auto_list
    )
}
