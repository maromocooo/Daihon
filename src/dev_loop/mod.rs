use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use chrono::Local;
use serde::{Deserialize, Serialize};

use crate::config::{DriftConfig, EnvironmentConfig};
use crate::model::scenario::{CheckState, Priority, Scenario, ScenarioStatus, StepStatus};
use crate::model::store::{LoadedScenario, load_scenarios_from_dir, load_scenarios_tree};
use crate::notify::Notifier;
use crate::runner::{AgentRequest, AgentRunner, ExecutionMode};
use crate::watcher::{
    ParentContext, WatchOptions, WatchRunReport, load_latest_run_for_scenario, watch_scenario,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MissingFeatureTask {
    pub task_id: String,
    pub task_file: PathBuf,
    pub scenario_id: String,
    pub scenario_title: String,
    pub priority: Priority,
    pub stopped_step_number: usize,
    pub stopped_step_title: String,
    pub failed_check_id: String,
    pub failed_check_description: String,
    pub failure_reason: String,
    pub markdown_body: String,
}

#[derive(Debug, Clone)]
pub struct DevLoopOptions {
    pub working_dir: PathBuf,
    pub scenarios_dir: PathBuf,
    pub state_dir: PathBuf,
    pub target_scenario_id: Option<String>,
    pub check_first: bool,
    pub plan_only: bool,
    pub auto_approve: bool,
    pub max_retries: usize,
    pub allow_same_agent: bool,
    pub watcher_sandbox: Option<String>,
    pub environment: EnvironmentConfig,
    pub drift: DriftConfig,
    pub timeout_secs: u64,
    pub developer_timeout_secs: u64,
    pub notify_topic: String,
    pub notify_on_ready_for_manual: bool,
}

#[derive(Debug, Clone)]
pub enum LoopOutcome {
    NoFailedScenario,
    PlannedOnly {
        task: MissingFeatureTask,
    },
    DeclinedByUser {
        task: MissingFeatureTask,
    },
    Resolved {
        scenario_id: String,
        final_status: ScenarioStatus,
        iterations: usize,
        tasks: Vec<MissingFeatureTask>,
        last_watch: WatchRunReport,
    },
    MaxRetriesReached {
        scenario_id: String,
        iterations: usize,
        tasks: Vec<MissingFeatureTask>,
        last_watch: WatchRunReport,
    },
}

/// Builds and persists a concrete "missing feature" task specification from a failed scenario step.
pub fn create_missing_feature_task(
    loaded: &LoadedScenario,
    latest_run: Option<&WatchRunReport>,
    state_dir: &Path,
) -> Result<MissingFeatureTask> {
    create_missing_feature_task_with_parent(loaded, None, latest_run, state_dir)
}

pub fn create_missing_feature_task_with_parent(
    loaded: &LoadedScenario,
    parent_scenario: Option<&Scenario>,
    latest_run: Option<&WatchRunReport>,
    state_dir: &Path,
) -> Result<MissingFeatureTask> {
    let sc = &loaded.scenario;
    let failed_step = sc
        .steps
        .iter()
        .find(|s| s.status == StepStatus::Failed)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "シナリオ `{}` に失敗中（failed）のステップがありません",
                sc.front_matter.id
            )
        })?;

    let failed_check = failed_step
        .failed_check
        .as_deref()
        .and_then(|fid| failed_step.checks.iter().find(|c| c.id == fid))
        .or_else(|| {
            failed_step
                .checks
                .iter()
                .find(|c| c.state == CheckState::Failed)
        })
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Step {} に失敗チェック項目が見つかりません",
                failed_step.number
            )
        })?;

    let failure_reason = failed_check
        .reason
        .clone()
        .unwrap_or_else(|| "チェック条件を満たしていません".to_string());

    let evidence_block = latest_run
        .and_then(|run| {
            run.step_results
                .iter()
                .flat_map(|s| s.checks.iter())
                .find(|c| c.check_id == failed_check.id)
        })
        .map(|rec| {
            format!(
                "- 実行コマンド: {}\n- 観測結果: {}",
                rec.command_run.as_deref().unwrap_or("（記録なし）"),
                rec.evidence.as_deref().unwrap_or("（記録なし）")
            )
        })
        .unwrap_or_else(|| "- 実行ログ: （見張り実行ログなし）".to_string());

    let timestamp = Local::now().format("%Y%m%d-%H%M%S-%3f").to_string();
    let task_id = format!("{}-{}-{}", timestamp, sc.front_matter.id, failed_check.id);

    let tasks_dir = state_dir.join("tasks");
    fs::create_dir_all(&tasks_dir)?;
    let task_file = tasks_dir.join(format!("{task_id}.md"));

    let parent_block = parent_scenario
        .map(|p| {
            format!(
                "\n## 親シナリオ（既に通過済みの本筋 — 壊さないこと）\n- **[{}] {}** (`{}`)\n{}\n",
                p.front_matter.priority, p.front_matter.title, p.front_matter.id, p.overview
            )
        })
        .unwrap_or_default();

    let markdown_body = format!(
        r#"# 足りない機能の実装タスク: [{priority}] {title}

- タスク ID: `{task_id}`
- 対象シナリオ: `{scenario_id}` ({scenario_path})
- 止まったステップ: Step {step_num}: {step_title}
- 失敗したチェック項目: `{check_id}` — {check_desc}
{parent_block}
## ユーザーの目的（シナリオ概要）
{overview}

## 見張り役が検知した不足・失敗内容
- 失敗理由: {failure_reason}
{evidence_block}

## 完成の条件
1. Step {step_num} のチェック項目 `{check_id}`（{check_desc}）が通るよう、足りない機能を実装すること。
2. Step 1〜{step_num} の既存チェック項目を壊さないこと。
3. 最小かつ自然な実装にとどめ、テストで動作を確認すること。
"#,
        priority = sc.front_matter.priority,
        title = sc.front_matter.title,
        scenario_id = sc.front_matter.id,
        scenario_path = loaded.path.display(),
        step_num = failed_step.number,
        step_title = failed_step.title,
        check_id = failed_check.id,
        check_desc = failed_check.description,
        overview = sc.overview,
    );

    fs::write(&task_file, &markdown_body)?;

    Ok(MissingFeatureTask {
        task_id,
        task_file,
        scenario_id: sc.front_matter.id.clone(),
        scenario_title: sc.front_matter.title.clone(),
        priority: sc.front_matter.priority,
        stopped_step_number: failed_step.number,
        stopped_step_title: failed_step.title.clone(),
        failed_check_id: failed_check.id.clone(),
        failed_check_description: failed_check.description.clone(),
        failure_reason,
        markdown_body,
    })
}

/// Runs the Stage 5 missing-feature feedback loop:
/// failed step -> task filing -> user approval gate -> developer agent -> distinct watcher re-check.
pub fn run_dev_loop<F>(
    developer_runner: &dyn AgentRunner,
    watcher_runner: &dyn AgentRunner,
    notifier: &dyn Notifier,
    options: &DevLoopOptions,
    mut approval_fn: F,
) -> Result<LoopOutcome>
where
    F: FnMut(&MissingFeatureTask) -> Result<bool>,
{
    if developer_runner.name() == watcher_runner.name() && !options.allow_same_agent {
        bail!(
            "開発役 ({}) と見張り役 ({}) は別のエージェントにする必要があります（--allow-same-agent で上書き可）",
            developer_runner.name(),
            watcher_runner.name()
        );
    }

    let watch_opts = WatchOptions {
        working_dir: options.working_dir.clone(),
        state_dir: options.state_dir.clone(),
        timeout_secs: options.timeout_secs,
        sandbox: options.watcher_sandbox.clone(),
        environment: options.environment.clone(),
        drift: options.drift.clone(),
        parent_context: None,
        notify_topic: options.notify_topic.clone(),
        notify_on_ready_for_manual: options.notify_on_ready_for_manual,
    };

    if options.check_first {
        let initial = load_scenarios_tree(&options.scenarios_dir)?;
        let mut satisfied: std::collections::HashMap<String, Scenario> =
            std::collections::HashMap::new();
        for entry in &initial {
            let item = &entry.loaded;
            if let Some(ref target) = options.target_scenario_id
                && item.scenario.front_matter.id != *target
            {
                if item.scenario.unlocks_children() {
                    satisfied.insert(item.scenario.front_matter.id.clone(), item.scenario.clone());
                }
                continue;
            }
            if options.target_scenario_id.is_none()
                && let Some(ref parent_id) = item.scenario.front_matter.parent
                && !satisfied.contains_key(parent_id)
            {
                continue;
            }
            let mut per_opts = watch_opts.clone();
            if let Some(ref parent_id) = item.scenario.front_matter.parent {
                per_opts.parent_context = satisfied
                    .get(parent_id)
                    .map(ParentContext::from_scenario);
            }
            let rep = watch_scenario(&item.path, watcher_runner, notifier, &per_opts)?;
            if matches!(
                rep.new_status,
                ScenarioStatus::Passed | ScenarioStatus::ReadyForManual
            ) && let Ok(reloaded) = crate::model::validate_file(&item.path)
            {
                satisfied.insert(reloaded.front_matter.id.clone(), reloaded);
            }
        }
    }

    let (mut current, parent_scenario) = select_failed_scenario(
        &options.scenarios_dir,
        options.target_scenario_id.as_deref(),
    )?
    .map_or((None, None), |(c, p)| (Some(c), p));
    let Some(mut current) = current.take() else {
        return Ok(LoopOutcome::NoFailedScenario);
    };

    let mut target_watch_opts = watch_opts;
    target_watch_opts.parent_context = parent_scenario.as_ref().map(ParentContext::from_scenario);

    let scenario_id = current.scenario.front_matter.id.clone();
    let scenario_path = current.path.clone();
    let mut created_tasks = Vec::new();
    let mut last_watch: Option<WatchRunReport> = None;
    let max_iterations = options.max_retries.max(1);

    for iteration in 1..=max_iterations {
        let latest_run = load_latest_run_for_scenario(&options.state_dir, &scenario_id)?;
        let task = create_missing_feature_task_with_parent(
            &current,
            parent_scenario.as_ref(),
            latest_run.as_ref(),
            &options.state_dir,
        )?;

        if options.plan_only {
            return Ok(LoopOutcome::PlannedOnly { task });
        }

        let approved = if options.auto_approve {
            true
        } else {
            approval_fn(&task)?
        };

        if !approved {
            return Ok(LoopOutcome::DeclinedByUser { task });
        }

        created_tasks.push(task.clone());

        // Hand off task to the Developer agent with write access.
        let dev_timeout = if options.developer_timeout_secs > 0 {
            options.developer_timeout_secs
        } else {
            options.timeout_secs
        };
        let dev_started = std::time::Instant::now();
        println!(
            "開発役 ({}) を起動しています (timeout: {}s)...",
            developer_runner.name(),
            dev_timeout
        );
        let dev_prompt = build_developer_prompt(&task);
        developer_runner.run(&AgentRequest {
            working_dir: options.working_dir.clone(),
            prompt: dev_prompt,
            json_schema: None,
            mode: ExecutionMode::WorkspaceWrite,
            sandbox_override: None,
            env_vars: options.environment.vars.clone(),
            timeout_secs: dev_timeout,
        })?;
        println!(
            "開発役 ({}) が完了しました ({}s)。見張り役 ({}) で再検証します...",
            developer_runner.name(),
            dev_started.elapsed().as_secs(),
            watcher_runner.name()
        );

        // Independently re-verify from Step 1 using the Watcher agent.
        let report = watch_scenario(&scenario_path, watcher_runner, notifier, &target_watch_opts)?;
        let resolved = matches!(
            report.new_status,
            ScenarioStatus::Passed | ScenarioStatus::ReadyForManual
        );
        last_watch = Some(report);

        if resolved {
            let final_watch = last_watch.expect("watch report exists");
            return Ok(LoopOutcome::Resolved {
                scenario_id,
                final_status: final_watch.new_status,
                iterations: iteration,
                tasks: created_tasks,
                last_watch: final_watch,
            });
        }

        // Reload updated scenario state for the next iteration.
        let refreshed = load_scenarios_from_dir(&options.scenarios_dir)?
            .into_iter()
            .find(|s| s.scenario.front_matter.id == scenario_id);
        match refreshed {
            Some(next) if next.scenario.front_matter.status == ScenarioStatus::Failed => {
                current = next;
            }
            _ => break,
        }
    }

    Ok(LoopOutcome::MaxRetriesReached {
        scenario_id,
        iterations: created_tasks.len(),
        tasks: created_tasks,
        last_watch: last_watch.expect("at least one iteration executed"),
    })
}

fn select_failed_scenario(
    scenarios_dir: &Path,
    target_id: Option<&str>,
) -> Result<Option<(LoadedScenario, Option<Scenario>)>> {
    let tree = load_scenarios_tree(scenarios_dir)?;
    let chosen = if let Some(tid) = target_id {
        tree.iter().find(|e| {
            e.loaded.scenario.front_matter.id == tid
                && e.loaded.scenario.front_matter.status == ScenarioStatus::Failed
        })
    } else {
        // Parent-first: prefer unblocked failed scenarios before child Togaki whose parent failed.
        tree.iter()
            .find(|e| {
                e.blocked_by_parent.is_none()
                    && e.loaded.scenario.front_matter.status == ScenarioStatus::Failed
            })
            .or_else(|| {
                tree.iter()
                    .find(|e| e.loaded.scenario.front_matter.status == ScenarioStatus::Failed)
            })
    };

    let Some(entry) = chosen else {
        return Ok(None);
    };
    let parent_sc = entry
        .loaded
        .scenario
        .front_matter
        .parent
        .as_deref()
        .and_then(|pid| {
            tree.iter()
                .find(|e| e.loaded.scenario.front_matter.id == pid)
                .map(|e| e.loaded.scenario.clone())
        });

    Ok(Some((entry.loaded.clone(), parent_sc)))
}

fn build_developer_prompt(task: &MissingFeatureTask) -> String {
    format!(
        r#"あなたはシナリオ駆動の開発ハーネスの「開発役エージェント」です。
見張り役の自動検証で足りない機能が見つかりました。以下のタスク仕様に従い、対象プロジェクトに足りない機能を実装してください。
完了後は見張り役（別エージェント）が Step 1 から再検証します。

---
{}
"#,
        task.markdown_body
    )
}
