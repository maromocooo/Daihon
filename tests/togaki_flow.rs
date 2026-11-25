use std::fs;

use daihon::config::{DriftConfig, EnvironmentConfig};
use daihon::model::{
    CheckState, Priority, ScenarioKind, ScenarioStatus, load_scenarios_tree, validate_path,
};
use daihon::notify::DryRunNotifier;
use daihon::register::{propose_togaki, save_registration};
use daihon::runner::AgentResponse;
use daihon::runner::mock::MockRunner;
use daihon::tui::app::App;
use daihon::tui::render::render;
use daihon::watcher::drift::{render_agent_context, sync_next_task_file};
use daihon::watcher::{ParentContext, WatchOptions, watch_scenario};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use serde_json::json;
use tempfile::TempDir;

#[test]
fn togaki_validator_rejects_missing_parent_self_parent_cycles_and_out_of_range_attach_step() {
    let dir = TempDir::new().unwrap();

    // Missing parent scenario in directory -> cross-file error.
    fs::write(
        dir.path().join("01-orphan.md"),
        r#"---
id: orphan-togaki
title: 親なしト書き
kind: togaki
parent: nonexistent-parent
priority: P1
status: pending
---

## 概要
親が存在しない。

## ステップ

### Step 1: チェック
- status: pending

- [ ] [auto] s1-c1: 条件を満たす
"#,
    )
    .unwrap();
    let report = validate_path(dir.path());
    assert!(!report.is_ok());
    assert!(report.cross_file_issues[0].message.contains("nonexistent-parent"));

    // Valid parent + attach_step exceeding parent step count -> cross-file error.
    let dir2 = TempDir::new().unwrap();
    fs::write(
        dir2.path().join("01-parent.md"),
        r#"---
id: seguito-ubuntu-blender
title: MacBook Air で seguito を開き Ubuntu の Blender で編集作業が行える
priority: P0
status: passed
hosts:
  - ubuntu-4090
---

## 概要
大きな本筋シナリオ。

## ステップ

### Step 1: Blender を起動して編集できる
- status: passed

- [x] [auto] s1-c1: Ubuntu 側 Blender セッションが起動して応答する
"#,
    )
    .unwrap();
    fs::write(
        dir2.path().join("02-bad-attach.md"),
        r#"---
id: bad-attach
title: ステップ番号超過
kind: togaki
parent: seguito-ubuntu-blender
attach_step: 5
priority: P1
status: pending
---

## 概要
親は 1 ステップしかないのに attach_step: 5。

## ステップ

### Step 1: チェック
- status: pending

- [ ] [auto] s1-c1: 遅延 50ms 以内
"#,
    )
    .unwrap();
    let report2 = validate_path(dir2.path());
    assert!(!report2.is_ok());
    assert!(report2.cross_file_issues[0].message.contains("attach_step"));

    // Cycle detection (togaki A -> togaki B -> togaki A).
    let dir3 = TempDir::new().unwrap();
    fs::write(
        dir3.path().join("01-a.md"),
        r#"---
id: cycle-a
title: 循環 A
kind: togaki
parent: cycle-b
priority: P1
status: pending
---

## 概要
A

## ステップ

### Step 1: A
- status: pending

- [ ] [auto] s1-c1: ok
"#,
    )
    .unwrap();
    fs::write(
        dir3.path().join("02-b.md"),
        r#"---
id: cycle-b
title: 循環 B
kind: togaki
parent: cycle-a
priority: P1
status: pending
---

## 概要
B

## ステップ

### Step 1: B
- status: pending

- [ ] [auto] s1-c1: ok
"#,
    )
    .unwrap();
    let report3 = validate_path(dir3.path());
    assert!(!report3.is_ok());
    assert!(report3.cross_file_issues.iter().any(|i| i.message.contains("循環")));
}

#[test]
fn togaki_end_to_end_registration_parent_gating_watcher_notification_and_tui_tree() {
    let dir = TempDir::new().unwrap();
    let scenarios_dir = dir.path().join("scenarios");
    let state_dir = dir.path().join(".daihon");
    fs::create_dir_all(&scenarios_dir).unwrap();

    // 1. Seed a large E2E parent scenario (initially FAILED so child Togaki starts blocked!).
    let parent_path = scenarios_dir.join("01-seguito-ubuntu-blender.md");
    let parent_content = r#"---
id: seguito-ubuntu-blender
title: MacBook Air で seguito を開き Ubuntu の Blender で編集作業が行える
priority: P0
status: failed
hosts:
  - ubuntu-4090
tags:
  - e2e
  - blender
---

## 概要
MacBook Air 上で seguito アプリを起動し、Ubuntu 端末で動作する Blender を開いて
そのまま 3D 編集作業・保存まで途切れずに行えること。

## ステップ

### Step 1: seguito から Ubuntu の Blender を起動できる
- status: failed
- failed_check: s1-c1

- [!] [auto] s1-c1: seguito 経由で Ubuntu 側の Blender プロセスが起動しセッションが確立する
  - reason: リモート起動ブリッジが未実装
"#;
    fs::write(&parent_path, parent_content).unwrap();
    let parent_bytes_before = fs::read(&parent_path).unwrap();

    // 2. Register 2 Togaki attached to `seguito-ubuntu-blender` via natural language!
    let mock_reg = MockRunner::with_json_responses(
        "mock-reg",
        vec![
            json!({
                "id": "seguito-blender-latency",
                "title": "遅延が 50ms 以内に収まる",
                "priority": "P1",
                "priority_rationale": "Blender 編集の操作感に直結する品質要件のため P1",
                "tags": ["latency", "performance"],
                "hosts": [],
                "overview": "seguito 経由で Ubuntu の Blender を操作したときの入力〜描画遅延を 50ms 以内に抑える。",
                "steps": [
                    {
                        "title": "操作遅延が 50ms 以内である",
                        "checks": [
                            { "id": "s1-c1", "mode": "auto", "description": "入力〜フレーム反映の p95 遅延が 50ms 以内である" }
                        ]
                    }
                ],
                "overlaps": []
            }),
            json!({
                "id": "seguito-blender-asset-transfer",
                "title": "Air のストレージ素材を seguito 上の Blender へ引き渡せる",
                "priority": "P1",
                "priority_rationale": "実際の制作ワークフローを完結させる主要派生機能のため P1",
                "tags": ["asset", "transfer"],
                "hosts": ["macbook-air"],
                "overview": "MacBook Air のローカルストレージにあるテクスチャ・モデル素材を、seguito で開いている Ubuntu 側 Blender へ引き渡して読み込める。",
                "steps": [
                    {
                        "title": "Air の素材を Ubuntu 側へ転送し Blender で読み込める",
                        "checks": [
                            { "id": "s1-c1", "mode": "auto", "description": "素材引き渡しコマンドで Ubuntu 側の作業ディレクトリへファイルが転送される" },
                            { "id": "s1-c2", "mode": "auto", "description": "Blender セッション内で転送済み素材が読み込まれエラーにならない" }
                        ]
                    }
                ],
                "overlaps": []
            }),
        ],
    );

    let prop_latency = propose_togaki(
        "seguito-ubuntu-blender",
        "遅延が 50ms 以内に収まること",
        None,
        &scenarios_dir,
        &mock_reg,
        dir.path(),
        60,
    )
    .expect("togaki 1 proposal");
    assert_eq!(prop_latency.scenario.front_matter.kind, ScenarioKind::Togaki);
    assert_eq!(
        prop_latency.scenario.front_matter.parent.as_deref(),
        Some("seguito-ubuntu-blender")
    );
    // Automatically inherits parent's `ubuntu-4090` host!
    assert!(
        prop_latency
            .scenario
            .front_matter
            .hosts
            .contains(&"ubuntu-4090".to_string())
    );
    let latency_path = save_registration(&prop_latency, &scenarios_dir, None).unwrap();

    let prop_asset = propose_togaki(
        "seguito-ubuntu-blender",
        "Air のストレージにある素材を seguito で開いている Blender に引き渡せること",
        Some(1),
        &scenarios_dir,
        &mock_reg,
        dir.path(),
        60,
    )
    .expect("togaki 2 proposal");
    assert_eq!(prop_asset.scenario.front_matter.attach_step, Some(1));
    assert!(
        prop_asset
            .scenario
            .front_matter
            .hosts
            .contains(&"ubuntu-4090".to_string())
    );
    assert!(
        prop_asset
            .scenario
            .front_matter
            .hosts
            .contains(&"macbook-air".to_string())
    );
    let _asset_path = save_registration(&prop_asset, &scenarios_dir, Some(Priority::P2)).unwrap();

    // Parent file MUST remain byte-for-byte intact!
    assert_eq!(fs::read(&parent_path).unwrap(), parent_bytes_before);
    assert!(validate_path(&scenarios_dir).is_ok());

    // 3. While parent is FAILED, both child Togaki are blocked by parent in tree, TUI, and NEXT_TASK!
    let tree_blocked = load_scenarios_tree(&scenarios_dir).unwrap();
    assert_eq!(tree_blocked.len(), 3);
    assert_eq!(tree_blocked[0].depth, 0);
    assert_eq!(tree_blocked[1].depth, 1);
    assert_eq!(
        tree_blocked[1].blocked_by_parent.as_deref(),
        Some("seguito-ubuntu-blender")
    );
    assert_eq!(tree_blocked[2].depth, 1);
    assert!(tree_blocked[2].is_last_sibling);

    let next_task = sync_next_task_file(&scenarios_dir, &state_dir, &DriftConfig::default()).unwrap();
    let next_body = fs::read_to_string(next_task).unwrap();
    assert!(next_body.contains("seguito-ubuntu-blender"));

    let ctx = render_agent_context(&scenarios_dir, &state_dir, &DriftConfig::default()).unwrap();
    assert!(ctx.contains("[ト書き]"));
    assert!(ctx.contains("親待ち: seguito-ubuntu-blender"));

    let app = App::load_with_state(&scenarios_dir, &state_dir, DriftConfig::default()).unwrap();
    let mut terminal = Terminal::new(TestBackend::new(130, 30)).unwrap();
    terminal.draw(|f| render(f, &app)).unwrap();
    let screen = terminal.backend().to_string();
    assert!(screen.contains("├─"), "ツリー枝が表示されるべき:\n{screen}");
    assert!(screen.contains("└─"), "末尾ツリー枝が表示されるべき:\n{screen}");
    assert!(screen.contains("[親待]"), "親待ちバッジが表示されるべき:\n{screen}");
    assert!(screen.contains("紐づくト書き: 2 件"), "親詳細に子ト書き集計が出るべき:\n{screen}");

    // 4. Mark parent as PASSED -> child Togaki immediately unlocks, receives ParentContext, and notifies on pass!
    let mut parent_sc = daihon::model::validate_file(&parent_path).unwrap();
    parent_sc.steps[0].checks[0].state = CheckState::Passed;
    parent_sc.steps[0].checks[0].reason = None;
    parent_sc.recompute_statuses();
    daihon::model::save_updated_scenario(&parent_path, &parent_sc).unwrap();

    let tree_unlocked = load_scenarios_tree(&scenarios_dir).unwrap();
    assert!(tree_unlocked[1].blocked_by_parent.is_none());

    let watcher = MockRunner::from_fn("codex", |req| {
        // Parent overview & host must be injected into watcher prompt!
        assert!(req.prompt.contains("seguito-ubuntu-blender"));
        assert!(req.prompt.contains("ubuntu-4090"));
        let body = json!({
            "checks": [{
                "check_id": "s1-c1",
                "passed": true,
                "command_run": "seguito bench --p95",
                "evidence": "p95=32ms",
                "reason": null
            }]
        });
        Ok(AgentResponse {
            agent_name: "codex".to_string(),
            raw_stdout: body.to_string(),
            raw_stderr: String::new(),
            structured_json: Some(body),
        })
    });

    let notifier = DryRunNotifier::new(state_dir.join("notifications.jsonl"));
    let watch_opts = WatchOptions {
        working_dir: dir.path().to_path_buf(),
        state_dir: state_dir.clone(),
        timeout_secs: 60,
        sandbox: Some("workspace-write".to_string()),
        environment: EnvironmentConfig::default(),
        drift: DriftConfig::default(),
        parent_context: Some(ParentContext::from_scenario(&parent_sc)),
        notify_topic: "my-topic".to_string(),
        notify_on_ready_for_manual: true,
    };

    let rep = watch_scenario(&latency_path, &watcher, &notifier, &watch_opts).unwrap();
    assert_eq!(rep.new_status, ScenarioStatus::Passed);
    assert!(rep.notified);

    let records = notifier.read_records().unwrap();
    assert_eq!(records.len(), 1);
    assert!(records[0].title.contains("ト書き通過"));
    assert!(records[0].message.contains("seguito-ubuntu-blender"));
}
