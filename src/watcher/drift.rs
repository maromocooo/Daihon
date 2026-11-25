use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::UNIX_EPOCH;

use anyhow::Result;
use chrono::Local;
use serde::{Deserialize, Serialize};

use crate::config::DriftConfig;
use crate::model::scenario::{CheckState, ScenarioStatus, StepStatus};
use crate::model::store::load_scenarios_from_dir;
use crate::watcher::WatchRunReport;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeSnapshot {
    pub git_head: Option<String>,
    pub fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DriftSummary {
    pub scenario_id: String,
    pub is_drifting: bool,
    pub consecutive_same_stop: usize,
    pub code_changes_without_progress: usize,
    pub stopped_step: usize,
    pub stopped_check: String,
    pub reason: String,
}

/// Computes a lightweight fingerprint of product source code state (excluding `scenarios/` and
/// `.daihon/` so watcher status write-backs never look like product code edits).
pub fn compute_worktree_fingerprint(
    workdir: &Path,
    scenarios_dir: Option<&Path>,
    state_dir: Option<&Path>,
) -> WorktreeSnapshot {
    let git_head = run_git(workdir, &["rev-parse", "HEAD"]);
    let porcelain = run_git(workdir, &["status", "--porcelain"]);

    let mut hash: u64 = 0xcbf29ce484222325;
    if let Some(ref head) = git_head {
        fnv1a_update(&mut hash, b"HEAD:");
        fnv1a_update(&mut hash, head.as_bytes());
    }

    let scenarios_rel = scenarios_dir.and_then(|p| p.strip_prefix(workdir).ok());
    let state_rel = state_dir.and_then(|p| p.strip_prefix(workdir).ok());

    if let Some(ref status_text) = porcelain {
        let mut dirty_lines: Vec<&str> = status_text.lines().collect();
        dirty_lines.sort();
        for line in dirty_lines {
            if line.len() < 4 {
                continue;
            }
            let rel_str = line[3..].trim();
            let rel_path = Path::new(rel_str);
            if is_ignored_harness_path(rel_path, scenarios_rel, state_rel) {
                continue;
            }
            fnv1a_update(&mut hash, line.as_bytes());
            fnv1a_update(&mut hash, b"|");
            let full = workdir.join(rel_path);
            hash_file_stat(&mut hash, &full);
        }
    } else {
        // Fallback for non-Git directories: scan up to 2 directory levels.
        let mut entries = BTreeSet::new();
        collect_shallow_files(workdir, workdir, 0, scenarios_rel, state_rel, &mut entries);
        for rel in entries {
            fnv1a_update(&mut hash, rel.to_string_lossy().as_bytes());
            fnv1a_update(&mut hash, b"|");
            hash_file_stat(&mut hash, &workdir.join(&rel));
        }
    }

    WorktreeSnapshot {
        git_head,
        fingerprint: format!("{hash:016x}"),
    }
}

/// Loads all historical `WatchRunReport`s for `scenario_id`, ordered oldest -> newest.
pub fn load_runs_for_scenario(state_dir: &Path, scenario_id: &str) -> Result<Vec<WatchRunReport>> {
    let runs_dir = state_dir.join("runs");
    if !runs_dir.exists() {
        return Ok(Vec::new());
    }

    let suffix = format!("-{scenario_id}.json");
    let mut paths: Vec<PathBuf> = fs::read_dir(&runs_dir)?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|name| name.ends_with(&suffix))
        })
        .collect();
    paths.sort();

    let mut reports = Vec::with_capacity(paths.len());
    for p in paths {
        if let Ok(content) = fs::read_to_string(&p)
            && let Ok(rep) = serde_json::from_str::<WatchRunReport>(&content)
        {
            reports.push(rep);
        }
    }
    Ok(reports)
}

/// Analyzes run history (optionally appending an in-flight `latest_override` report)
/// to determine whether the scenario is stuck/drifting on the exact same step/check.
pub fn analyze_drift_from_reports(
    reports: &[WatchRunReport],
    cfg: &DriftConfig,
) -> Option<DriftSummary> {
    let latest = reports.last()?;
    if latest.new_status != ScenarioStatus::Failed {
        return None;
    }
    let stopped_step = latest.stopped_step?;
    let stopped_check = latest.stopped_check.clone()?;

    let mut consecutive_same_stop = 0usize;
    let mut code_changes_without_progress = 0usize;
    let mut next_newer_fp: Option<&str> = None;

    for rep in reports.iter().rev() {
        if rep.new_status != ScenarioStatus::Failed
            || rep.stopped_step != Some(stopped_step)
            || rep.stopped_check.as_deref() != Some(stopped_check.as_str())
        {
            break;
        }
        consecutive_same_stop += 1;

        if let (Some(newer_fp), Some(cur_fp)) =
            (next_newer_fp, rep.worktree_fingerprint.as_deref())
            && newer_fp != cur_fp
        {
            code_changes_without_progress += 1;
        }
        if let Some(cur_fp) = rep.worktree_fingerprint.as_deref() {
            next_newer_fp = Some(cur_fp);
        }
    }

    let stagnant_thresh = cfg.stagnant_runs_threshold.max(1);
    let code_thresh = cfg.code_changes_threshold.max(1);
    let is_drifting = consecutive_same_stop >= stagnant_thresh
        || code_changes_without_progress >= code_thresh;

    let reason = if code_changes_without_progress > 0 {
        format!(
            "{consecutive_same_stop} 回連続（うちコード変更 {code_changes_without_progress} 回）で Step {stopped_step} ({stopped_check}) から前進していません"
        )
    } else {
        format!(
            "{consecutive_same_stop} 回連続で Step {stopped_step} ({stopped_check}) に停止しています"
        )
    };

    Some(DriftSummary {
        scenario_id: latest.scenario_id.clone(),
        is_drifting,
        consecutive_same_stop,
        code_changes_without_progress,
        stopped_step,
        stopped_check,
        reason,
    })
}

pub fn analyze_scenario_drift(
    state_dir: &Path,
    scenario_id: &str,
    cfg: &DriftConfig,
) -> Result<Option<DriftSummary>> {
    let reports = load_runs_for_scenario(state_dir, scenario_id)?;
    Ok(analyze_drift_from_reports(&reports, cfg))
}

/// Automatically writes `.daihon/NEXT_TASK.md` summarizing the highest-priority
/// failed scenario (plus any drift warning) so coding agents immediately see what to fix next.
pub fn sync_next_task_file(
    scenarios_dir: &Path,
    state_dir: &Path,
    drift_cfg: &DriftConfig,
) -> Result<PathBuf> {
    fs::create_dir_all(state_dir)?;
    let next_task_path = state_dir.join("NEXT_TASK.md");
    let loaded = load_scenarios_from_dir(scenarios_dir)?;
    let now = Local::now().to_rfc3339();

    let top_failed = loaded
        .iter()
        .find(|item| item.scenario.front_matter.status == ScenarioStatus::Failed);

    let body = match top_failed {
        None => {
            let pending_count = loaded
                .iter()
                .filter(|s| s.scenario.front_matter.status == ScenarioStatus::Pending)
                .count();
            let ready_count = loaded
                .iter()
                .filter(|s| s.scenario.front_matter.status == ScenarioStatus::ReadyForManual)
                .count();
            format!(
                "# 現在の最優先タスク（daihon）\n\n\
                 - 更新日時: {now}\n\
                 - 状態: 現在、失敗中（`failed`）のシナリオはありません（確認待ち: {ready_count} 件 / 未検証: {pending_count} 件）。\n"
            )
        }
        Some(item) => {
            let sc = &item.scenario;
            let failed_step = sc.steps.iter().find(|s| s.status == StepStatus::Failed);
            let drift = analyze_scenario_drift(state_dir, &sc.front_matter.id, drift_cfg)?;

            let drift_section = match &drift {
                Some(d) if d.is_drifting => format!(
                    "\n> [!WARNING]\n> **ドリフト警告**: {}\n> 単体パーツの作業を止め、このステップを通す機能を最優先で実装してください。\n",
                    d.reason
                ),
                Some(d) => format!("\n- 停滞状況: {}\n", d.reason),
                None => String::new(),
            };

            let (step_line, check_line, reason_line) = if let Some(step) = failed_step {
                let failed_check = step
                    .failed_check
                    .as_deref()
                    .and_then(|fid| step.checks.iter().find(|c| c.id == fid))
                    .or_else(|| step.checks.iter().find(|c| c.state == CheckState::Failed));
                let step_desc = format!("Step {}: {}", step.number, step.title);
                let (chk, rsn) = match failed_check {
                    Some(c) => (
                        format!("`{}` — {}", c.id, c.description),
                        c.reason
                            .clone()
                            .unwrap_or_else(|| "検証条件を満たしていません".to_string()),
                    ),
                    None => ("(不明)".to_string(), "失敗理由なし".to_string()),
                };
                (step_desc, chk, rsn)
            } else {
                (
                    "(不明)".to_string(),
                    "(不明)".to_string(),
                    "失敗理由なし".to_string(),
                )
            };

            format!(
                "# 現在の最優先タスク（daihon 自動生成）\n\n\
                 - 更新日時: {now}\n\
                 - 最優先停止シナリオ: **[{}] {}** (`{}`)\n\
                 - シナリオファイル: `{}`\n\
                 - 止まったステップ: {}\n\
                 - 失敗したチェック項目: {}\n\
                 - 失敗理由: {}\n\
                 {drift_section}\n\
                 ## ユーザーが実現したい使い方（概要）\n\
                 {}\n\n\
                 ## 開発役エージェントへの指示\n\
                 1. 単体パーツより先に、上記の失敗チェック項目を通すための不足機能を実装してください。\n\
                 2. ユーザーから指摘される前に「最優先シナリオの {} を通すため、この機能を作りましょう」と提案・着手してください。\n\
                 3. 実装後は `daihon watch --scenario {}` で別エージェント検証を通るか確認してください。\n",
                sc.front_matter.priority,
                sc.front_matter.title,
                sc.front_matter.id,
                item.path.display(),
                step_line,
                check_line,
                reason_line,
                sc.overview,
                step_line,
                sc.front_matter.id,
            )
        }
    };

    fs::write(&next_task_path, body)?;
    Ok(next_task_path)
}

/// Renders a concise brief suitable for injecting into a coding agent session (`SessionStart` hook
/// or `daihon context`). Returns empty string if no scenarios exist yet.
pub fn render_agent_context(
    scenarios_dir: &Path,
    state_dir: &Path,
    drift_cfg: &DriftConfig,
) -> Result<String> {
    let loaded = load_scenarios_from_dir(scenarios_dir)?;
    if loaded.is_empty() {
        return Ok(String::new());
    }

    let mut lines = Vec::new();
    lines.push("[daihon] 現在の登録シナリオ状況（優先度順）:".to_string());

    let mut drifting_alerts = Vec::new();
    let mut top_failed_id: Option<String> = None;

    for item in loaded.iter().take(5) {
        let sc = &item.scenario;
        let drift = analyze_scenario_drift(state_dir, &sc.front_matter.id, drift_cfg)?;
        let drift_tag = match &drift {
            Some(d) if d.is_drifting => {
                drifting_alerts.push(format!(
                    "- [{}] {}: {}",
                    sc.front_matter.priority, sc.front_matter.id, d.reason
                ));
                format!(" [DRIFT x{}]", d.consecutive_same_stop)
            }
            _ => String::new(),
        };

        let stop_info = if sc.front_matter.status == ScenarioStatus::Failed {
            if top_failed_id.is_none() {
                top_failed_id = Some(sc.front_matter.id.clone());
            }
            sc.steps
                .iter()
                .find(|s| s.status == StepStatus::Failed)
                .map(|s| {
                    format!(
                        " (停止: Step {} / {})",
                        s.number,
                        s.failed_check.as_deref().unwrap_or("-")
                    )
                })
                .unwrap_or_default()
        } else {
            String::new()
        };

        lines.push(format!(
            "- [{}] {} `{}`: {}{}{}",
            sc.front_matter.priority,
            sc.front_matter.status,
            sc.front_matter.id,
            sc.front_matter.title,
            stop_info,
            drift_tag
        ));
    }

    if !drifting_alerts.is_empty() {
        lines.push(String::new());
        lines.push("⚠ ドリフト警告（同じステップで停滞中）:".to_string());
        lines.extend(drifting_alerts);
    }

    if top_failed_id.is_some() {
        let next_task = state_dir.join("NEXT_TASK.md");
        lines.push(String::new());
        lines.push(format!(
            "方針: 単体パーツより最優先シナリオの停止ステップ解消を優先してください。詳細: {}",
            next_task.display()
        ));
    }

    Ok(lines.join("\n"))
}

fn run_git(workdir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(workdir)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn is_ignored_harness_path(
    rel: &Path,
    scenarios_rel: Option<&Path>,
    state_rel: Option<&Path>,
) -> bool {
    if let Some(sc) = scenarios_rel
        && rel.starts_with(sc)
    {
        return true;
    }
    if let Some(st) = state_rel
        && rel.starts_with(st)
    {
        return true;
    }
    rel.starts_with("scenarios")
        || rel.starts_with(".daihon")
        || rel.starts_with(".git")
        || rel.starts_with("target")
        || rel.starts_with("node_modules")
}

fn collect_shallow_files(
    root: &Path,
    dir: &Path,
    depth: usize,
    scenarios_rel: Option<&Path>,
    state_rel: Option<&Path>,
    out: &mut BTreeSet<PathBuf>,
) {
    if depth > 2 {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let Ok(rel) = path.strip_prefix(root) else {
            continue;
        };
        if is_ignored_harness_path(rel, scenarios_rel, state_rel) {
            continue;
        }
        if path.is_dir() {
            collect_shallow_files(root, &path, depth + 1, scenarios_rel, state_rel, out);
        } else if path.is_file() {
            out.insert(rel.to_path_buf());
        }
    }
}

fn hash_file_stat(hash: &mut u64, path: &Path) {
    if let Ok(meta) = fs::metadata(path) {
        fnv1a_update(hash, &meta.len().to_le_bytes());
        if let Ok(modified) = meta.modified()
            && let Ok(dur) = modified.duration_since(UNIX_EPOCH)
        {
            fnv1a_update(hash, &dur.as_nanos().to_le_bytes());
        }
    }
}

fn fnv1a_update(hash: &mut u64, bytes: &[u8]) {
    for &b in bytes {
        *hash ^= u64::from(b);
        *hash = hash.wrapping_mul(0x100000001b3);
    }
}
