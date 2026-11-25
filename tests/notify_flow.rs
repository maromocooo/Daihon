use std::fs;

use daihon::config::{DriftConfig, EnvironmentConfig};
use daihon::model::{Priority, ScenarioStatus};
use daihon::notify::{
    DryRunNotifier, NotificationPayload, build_ntfy_json_body, normalize_ntfy_topic, should_notify,
};
use daihon::runner::mock::MockRunner;
use daihon::watcher::{WatchOptions, watch_scenario};
use serde_json::json;
use tempfile::TempDir;

#[test]
fn notify_rule_never_fires_on_partial_steps_and_fires_once_on_full_pass() {
    // Pure unit checks of notification state machine.
    assert!(!should_notify(
        ScenarioStatus::Pending,
        ScenarioStatus::InProgress,
        None,
        true
    ));
    assert!(!should_notify(
        ScenarioStatus::Pending,
        ScenarioStatus::Failed,
        None,
        true
    ));
    assert!(should_notify(
        ScenarioStatus::Failed,
        ScenarioStatus::Passed,
        None,
        true
    ));
    assert!(!should_notify(
        ScenarioStatus::Passed,
        ScenarioStatus::Passed,
        Some("2026-11-25T10:00:00+09:00"),
        true
    ));
    assert!(should_notify(
        ScenarioStatus::Failed,
        ScenarioStatus::ReadyForManual,
        None,
        true
    ));
    assert!(!should_notify(
        ScenarioStatus::Failed,
        ScenarioStatus::ReadyForManual,
        None,
        false
    ));
}

#[test]
fn dry_run_notifier_records_only_on_all_steps_passed_transition() {
    let dir = TempDir::new().unwrap();
    let scenario_path = dir.path().join("01-demo.md");
    fs::write(
        &scenario_path,
        r#"---
id: notify-demo
title: 通知テスト用シナリオ
priority: P0
status: pending
last_checked_at: null
last_checked_by: null
notified_pass_at: null
---

## 概要
全ステップ通過時だけスマホ通知が飛ぶことを検証する。

## ステップ

### Step 1: 最初のステップ
- status: pending

- [ ] [auto] s1-c1: 準備できる

### Step 2: 仕上げのステップ
- status: pending

- [ ] [auto] s2-c1: 完了できる
"#,
    )
    .unwrap();

    let state_dir = dir.path().join(".daihon");
    let notifier = DryRunNotifier::new(state_dir.join("notifications.jsonl"));
    let opts = WatchOptions {
        working_dir: dir.path().to_path_buf(),
        state_dir,
        timeout_secs: 30,
        sandbox: None,
        environment: EnvironmentConfig::default(),
        drift: DriftConfig::default(),
        parent_context: None,
        notify_topic: "my-secret-topic".to_string(),
        notify_on_ready_for_manual: true,
    };

    // Run 1: Step 1 passes, Step 2 fails -> MUST NOT notify!
    let watcher_fail = MockRunner::with_json_responses(
        "codex",
        vec![
            json!({ "checks": [{ "check_id": "s1-c1", "passed": true, "command_run": "cmd1", "evidence": "ok", "reason": null }] }),
            json!({ "checks": [{ "check_id": "s2-c1", "passed": false, "command_run": "cmd2", "evidence": "err", "reason": "未実装" }] }),
        ],
    );
    let r1 = watch_scenario(&scenario_path, &watcher_fail, &notifier, &opts).unwrap();
    assert!(!r1.notified);
    assert_eq!(notifier.read_records().unwrap().len(), 0);

    // Run 2: Both Step 1 and Step 2 pass -> MUST notify ONCE!
    let watcher_pass = MockRunner::with_json_responses(
        "codex",
        vec![
            json!({ "checks": [{ "check_id": "s1-c1", "passed": true, "command_run": "cmd1", "evidence": "ok", "reason": null }] }),
            json!({ "checks": [{ "check_id": "s2-c1", "passed": true, "command_run": "cmd2", "evidence": "ok", "reason": null }] }),
        ],
    );
    let r2 = watch_scenario(&scenario_path, &watcher_pass, &notifier, &opts).unwrap();
    assert!(r2.notified);
    assert_eq!(notifier.read_records().unwrap().len(), 1);

    // Run 3: Re-running watcher when already passed -> MUST NOT send duplicate notification!
    let watcher_pass_again = MockRunner::with_json_responses(
        "codex",
        vec![
            json!({ "checks": [{ "check_id": "s1-c1", "passed": true, "command_run": "cmd1", "evidence": "ok", "reason": null }] }),
            json!({ "checks": [{ "check_id": "s2-c1", "passed": true, "command_run": "cmd2", "evidence": "ok", "reason": null }] }),
        ],
    );
    let r3 = watch_scenario(&scenario_path, &watcher_pass_again, &notifier, &opts).unwrap();
    assert!(!r3.notified);
    assert_eq!(notifier.read_records().unwrap().len(), 1);
}

#[test]
fn ntfy_topic_normalization_and_utf8_japanese_json_body() {
    assert_eq!(
        normalize_ntfy_topic("my-topic", "https://ntfy.sh"),
        "my-topic"
    );
    assert_eq!(
        normalize_ntfy_topic("ntfy.sh/my-topic", "https://ntfy.sh"),
        "my-topic"
    );
    assert_eq!(
        normalize_ntfy_topic("https://ntfy.sh/my-topic/", "https://ntfy.sh"),
        "my-topic"
    );

    let payload = NotificationPayload {
        scenario_id: "notify-test".to_string(),
        title: "[TEST] Daihon 通知テスト".to_string(),
        priority: Priority::P0,
        status: ScenarioStatus::Passed,
        checked_at: "2026-11-25T16:53:00+09:00".to_string(),
        message: "全ステップ通過しました".to_string(),
        topic: "ntfy.sh/my-topic".to_string(),
    };

    let body = build_ntfy_json_body(&payload, "https://ntfy.sh");
    assert_eq!(body["topic"], "my-topic");
    assert_eq!(body["title"], "[TEST] Daihon 通知テスト");
    assert_eq!(body["priority"], 4);
}
