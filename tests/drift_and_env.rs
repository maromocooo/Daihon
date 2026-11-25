use std::collections::BTreeMap;
use std::fs;

use daihon::config::{DriftConfig, EnvironmentConfig};
use daihon::model::{ScenarioStatus, validate_str};
use daihon::notify::NoopNotifier;
use daihon::runner::AgentResponse;
use daihon::runner::mock::MockRunner;
use daihon::watcher::drift::{
    analyze_scenario_drift, compute_worktree_fingerprint, render_agent_context,
};
use daihon::watcher::trigger::{
    is_unchanged_since_last_watch, is_within_hook_cooldown, record_hook_trigger,
    record_last_watch_fingerprint,
};
use daihon::watcher::{WatchOptions, watch_scenario};
use serde_json::json;
use tempfile::TempDir;

#[test]
fn drift_triggers_after_stagnant_runs_updates_next_task_and_clears_when_resolved() {
    let dir = TempDir::new().unwrap();
    let scenarios_dir = dir.path().join("scenarios");
    let state_dir = dir.path().join(".daihon");
    fs::create_dir_all(&scenarios_dir).unwrap();

    // Product source file to simulate code changes between watcher runs.
    let src_file = dir.path().join("app.txt");
    fs::write(&src_file, "v1: text editor\n").unwrap();

    let scenario_path = scenarios_dir.join("01-core.md");
    fs::write(
        &scenario_path,
        r#"---
id: rio-herdr-core
title: Rio から herdr でマルチマシン開発
priority: P0
status: pending
hosts:
  - mac-studio
  - ubuntu-4090
---

## 概要
3 台をまたいで開発を続ける中核シナリオ。

## ステップ

### Step 1: 起動できる
- status: pending

- [ ] [auto] s1-c1: herdr が起動する

### Step 2: Ubuntu へ引き継げる
- status: pending

- [ ] [auto] s2-c1: Ubuntu へセッション引き継ぎできる
"#,
    )
    .unwrap();

    let mut env_vars = BTreeMap::new();
    env_vars.insert("HERDR_ENV".to_string(), "dogfood".to_string());
    let env_cfg = EnvironmentConfig {
        notes: "Mac Studio は ssh mac-studio、Ubuntu は ssh ubuntu-4090 で接続する".to_string(),
        hosts: vec!["mac-studio".to_string()],
        setup_commands: vec!["herdr doctor".to_string()],
        vars: env_vars,
    };

    let opts = WatchOptions {
        working_dir: dir.path().to_path_buf(),
        state_dir: state_dir.clone(),
        timeout_secs: 60,
        sandbox: Some("workspace-write".to_string()),
        environment: env_cfg,
        drift: DriftConfig {
            stagnant_runs_threshold: 3,
            code_changes_threshold: 2,
        },
        parent_context: None,
        notify_topic: "test".to_string(),
        notify_on_ready_for_manual: true,
    };

    let fail_watcher = MockRunner::from_fn("codex", |req| {
        let body = if req.prompt.contains("Step 1:") {
            json!({ "checks": [{ "check_id": "s1-c1", "passed": true, "command_run": "herdr --version", "evidence": "ok", "reason": null }] })
        } else {
            json!({ "checks": [{ "check_id": "s2-c1", "passed": false, "command_run": "herdr handoff", "evidence": "not found", "reason": "handoff 未実装" }] })
        };
        Ok(AgentResponse {
            agent_name: "codex".to_string(),
            raw_stdout: body.to_string(),
            raw_stderr: String::new(),
            structured_json: Some(body),
        })
    });

    // Run 1: fails at Step 2 (s2-c1) -> consecutive = 1, not yet drifting.
    let r1 = watch_scenario(&scenario_path, &fail_watcher, &NoopNotifier, &opts).unwrap();
    assert_eq!(r1.new_status, ScenarioStatus::Failed);
    let d1 = r1.drift.expect("drift summary present");
    assert!(!d1.is_drifting);
    assert_eq!(d1.consecutive_same_stop, 1);

    // Verify environment hints and scenario hosts were injected into watcher prompt & env_vars!
    let calls = fail_watcher.recorded_calls();
    assert!(calls[0].prompt.contains("mac-studio"));
    assert!(calls[0].prompt.contains("ubuntu-4090"));
    assert!(calls[0].prompt.contains("herdr doctor"));
    assert!(calls[0].prompt.contains("ssh ubuntu-4090"));
    assert_eq!(
        calls[0].env_vars.get("HERDR_ENV").map(String::as_str),
        Some("dogfood")
    );

    // Check NEXT_TASK.md was automatically written!
    let next_task = fs::read_to_string(state_dir.join("NEXT_TASK.md")).unwrap();
    assert!(next_task.contains("rio-herdr-core"));
    assert!(next_task.contains("s2-c1"));

    // Run 2 (after unrelated code edit): consecutive = 2, code_changes = 1 -> still below threshold.
    fs::write(&src_file, "v2: text editor + firefox\n").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    let r2 = watch_scenario(&scenario_path, &fail_watcher, &NoopNotifier, &opts).unwrap();
    let d2 = r2.drift.expect("drift summary 2");
    assert_eq!(d2.consecutive_same_stop, 2);

    // Run 3 (after another unrelated code edit): consecutive = 3 -> DRIFTING!
    fs::write(&src_file, "v3: firefox theme tweaks\n").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    let r3 = watch_scenario(&scenario_path, &fail_watcher, &NoopNotifier, &opts).unwrap();
    let d3 = r3.drift.expect("drift summary 3");
    assert!(d3.is_drifting, "3回連続同一ステップ停止でドリフト検知されるべき: {d3:?}");
    assert_eq!(d3.consecutive_same_stop, 3);
    assert!(d3.code_changes_without_progress >= 2);

    // NEXT_TASK.md and agent context now carry explicit drift warning!
    let next_task_drift = fs::read_to_string(state_dir.join("NEXT_TASK.md")).unwrap();
    assert!(next_task_drift.contains("ドリフト警告"));
    let ctx = render_agent_context(&scenarios_dir, &state_dir, &opts.drift).unwrap();
    assert!(ctx.contains("[DRIFT x3]"));
    assert!(ctx.contains("NEXT_TASK.md"));

    // Run 4: now all steps pass -> drift clears and NEXT_TASK.md reports no failed scenarios!
    let pass_watcher = MockRunner::with_json_responses(
        "codex",
        vec![
            json!({ "checks": [{ "check_id": "s1-c1", "passed": true, "command_run": "herdr --version", "evidence": "ok", "reason": null }] }),
            json!({ "checks": [{ "check_id": "s2-c1", "passed": true, "command_run": "herdr handoff", "evidence": "ok", "reason": null }] }),
        ],
    );
    let r4 = watch_scenario(&scenario_path, &pass_watcher, &NoopNotifier, &opts).unwrap();
    assert_eq!(r4.new_status, ScenarioStatus::Passed);
    assert!(r4.drift.is_none());
    assert!(
        analyze_scenario_drift(&state_dir, "rio-herdr-core", &opts.drift)
            .unwrap()
            .is_none()
    );
    let next_task_clear = fs::read_to_string(state_dir.join("NEXT_TASK.md")).unwrap();
    assert!(next_task_clear.contains("失敗中（`failed`）のシナリオ／ト書きはありません"));
}

#[test]
fn worktree_fingerprint_ignores_scenario_updates_and_detects_code_edits() {
    let dir = TempDir::new().unwrap();
    let scenarios_dir = dir.path().join("scenarios");
    let state_dir = dir.path().join(".daihon");
    fs::create_dir_all(&scenarios_dir).unwrap();
    fs::create_dir_all(&state_dir).unwrap();

    let src = dir.path().join("main.rs");
    fs::write(&src, "fn main() {}\n").unwrap();

    let fp1 = compute_worktree_fingerprint(dir.path(), Some(&scenarios_dir), Some(&state_dir));
    record_last_watch_fingerprint(&state_dir, &fp1.fingerprint).unwrap();
    assert!(is_unchanged_since_last_watch(
        dir.path(),
        &scenarios_dir,
        &state_dir
    ));

    // Updating a scenario file must NOT be treated as a product code change.
    fs::write(scenarios_dir.join("01-test.md"), "updated status\n").unwrap();
    assert!(is_unchanged_since_last_watch(
        dir.path(),
        &scenarios_dir,
        &state_dir
    ));

    // Editing product source code MUST flip unchanged -> false.
    fs::write(&src, "fn main() { println!(\"changed\"); }\n").unwrap();
    assert!(!is_unchanged_since_last_watch(
        dir.path(),
        &scenarios_dir,
        &state_dir
    ));

    // Hook cooldown check.
    assert!(!is_within_hook_cooldown(&state_dir, 60));
    record_hook_trigger(&state_dir).unwrap();
    assert!(is_within_hook_cooldown(&state_dir, 60));
    assert!(!is_within_hook_cooldown(&state_dir, 0));
}

#[test]
fn scenario_hosts_field_parses_and_validates() {
    let md = r#"---
id: multi-host-check
title: マルチホスト検証
priority: P0
status: pending
hosts:
  - mac-studio
  - ubuntu-4090
---

## 概要
複数マシンにまたがるシナリオ。

## ステップ

### Step 1: 疎通
- status: pending

- [ ] [auto] s1-c1: 両ホストへ SSH できる
"#;
    let sc = validate_str(md, None).expect("hosts front matter should validate");
    assert_eq!(sc.front_matter.hosts, vec!["mac-studio", "ubuntu-4090"]);
}
