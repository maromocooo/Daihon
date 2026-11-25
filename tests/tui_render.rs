use std::fs;
use std::path::Path;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use daihon::tui::app::App;
use daihon::tui::render::render;
use tempfile::TempDir;

fn buffer_to_string(terminal: &Terminal<TestBackend>) -> String {
    terminal.backend().to_string()
}

#[test]
fn tui_distinguishes_passed_failed_ready_for_manual_and_pending_scenarios() {
    let dir = TempDir::new().unwrap();

    fs::write(
        dir.path().join("01-passed.md"),
        r#"---
id: all-passed
title: 全通シナリオ
priority: P1
status: passed
last_checked_at: 2026-11-25T10:00:00+09:00
last_checked_by: codex
---

## 概要
全部通った見本。

## ステップ

### Step 1: 完了ステップ
- status: passed

- [x] [auto] s1-c1: 正常終了する
"#,
    )
    .unwrap();

    fs::write(
        dir.path().join("02-failed.md"),
        r#"---
id: stopped-midway
title: 途中停止シナリオ
priority: P0
status: failed
last_checked_at: 2026-11-25T10:05:00+09:00
last_checked_by: codex
---

## 概要
Step 2 で止まった見本。

## ステップ

### Step 1: 準備
- status: passed

- [x] [auto] s1-c1: 初期化できる

### Step 2: 引き継ぎ
- status: failed
- failed_check: s2-c1

- [!] [auto] s2-c1: Ubuntu へ引き継げる
  - reason: handoff サブコマンドが未実装
"#,
    )
    .unwrap();

    fs::write(
        dir.path().join("03-wait.md"),
        r#"---
id: waiting-manual
title: 実機確認待ちシナリオ
priority: P2
status: ready_for_manual
---

## 概要
自動部分はすべて通過。

## ステップ

### Step 1: 実機確認
- status: ready_for_manual

- [x] [auto] s1-c1: 起動コマンド OK
- [ ] [manual] s1-c2: Rio 上で描画崩れがない
"#,
    )
    .unwrap();

    fs::write(
        dir.path().join("04-pending.md"),
        r#"---
id: not-yet-run
title: 未検証シナリオ
priority: P3
status: pending
---

## 概要
まだ一度も試していない見本。

## ステップ

### Step 1: 未実行
- status: pending

- [ ] [auto] s1-c1: 実行できる
"#,
    )
    .unwrap();

    let mut app = App::load(dir.path()).expect("App::load should succeed");
    // Priority sort puts P0 (stopped-midway) first.
    assert_eq!(
        app.selected_scenario().unwrap().scenario.front_matter.id,
        "stopped-midway"
    );

    let backend = TestBackend::new(120, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| render(f, &app)).unwrap();

    let screen = buffer_to_string(&terminal);
    assert!(screen.contains("[FAIL]"), "FAIL バッジが表示されるべき:\n{screen}");
    assert!(screen.contains("[PASS]"), "PASS バッジが表示されるべき:\n{screen}");
    assert!(screen.contains("[WAIT]"), "WAIT バッジが表示されるべき:\n{screen}");
    assert!(screen.contains("[TODO]"), "TODO バッジが表示されるべき:\n{screen}");
    assert!(
        screen.contains("handoff サブコマンドが未実装"),
        "選択中シナリオの停止理由が表示されるべき:\n{screen}"
    );

    // Move selection to next scenario (P1: all-passed) and re-render.
    app.next();
    assert_eq!(
        app.selected_scenario().unwrap().scenario.front_matter.id,
        "all-passed"
    );
    terminal.draw(|f| render(f, &app)).unwrap();
    let screen2 = buffer_to_string(&terminal);
    assert!(screen2.contains("all-passed"));
    assert!(screen2.contains("完了ステップ"));
}

#[test]
fn tui_renders_repository_sample_scenarios() {
    let app = App::load(Path::new("examples/scenarios")).expect("load sample scenarios");
    let backend = TestBackend::new(120, 32);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| render(f, &app)).unwrap();

    let screen = buffer_to_string(&terminal);
    assert!(screen.contains("Scenarios (3)"));
    assert!(screen.contains("rio-herdr-cross-machine"));
    assert!(screen.contains("Step 2 [FAIL]"));
    assert!(screen.contains("[ト書き]"));
}

#[test]
fn tui_displays_drift_badge_and_banner_when_scenario_stagnates() {
    use daihon::config::DriftConfig;
    use daihon::model::ScenarioStatus;
    use daihon::watcher::WatchRunReport;

    let dir = TempDir::new().unwrap();
    let scenarios_dir = dir.path().join("scenarios");
    let state_dir = dir.path().join(".daihon");
    let runs_dir = state_dir.join("runs");
    fs::create_dir_all(&scenarios_dir).unwrap();
    fs::create_dir_all(&runs_dir).unwrap();

    fs::copy(
        "examples/scenarios/01-rio-herdr-cross-machine.md",
        scenarios_dir.join("01-rio-herdr-cross-machine.md"),
    )
    .unwrap();

    for i in 1..=3 {
        let rep = WatchRunReport {
            run_id: format!("20261125-20000{i}-000"),
            scenario_id: "rio-herdr-cross-machine".to_string(),
            scenario_path: scenarios_dir.join("01-rio-herdr-cross-machine.md"),
            watcher_agent: "codex".to_string(),
            started_at: "2026-11-25T20:00:00+09:00".to_string(),
            finished_at: "2026-11-25T20:00:10+09:00".to_string(),
            previous_status: ScenarioStatus::Failed,
            new_status: ScenarioStatus::Failed,
            stopped_step: Some(2),
            stopped_check: Some("s2-c2".to_string()),
            stopped_reason: Some("未実装".to_string()),
            step_results: Vec::new(),
            notified: false,
            git_head: None,
            worktree_fingerprint: Some(format!("fp-{i}")),
            drift: None,
            report_path: runs_dir.join(format!("20261125-20000{i}-000-rio-herdr-cross-machine.json")),
        };
        fs::write(&rep.report_path, serde_json::to_string_pretty(&rep).unwrap()).unwrap();
    }

    let app = App::load_with_state(&scenarios_dir, &state_dir, DriftConfig::default()).unwrap();
    let mut terminal = Terminal::new(TestBackend::new(130, 34)).unwrap();
    terminal.draw(|f| render(f, &app)).unwrap();

    let screen = buffer_to_string(&terminal);
    assert!(screen.contains("[DRIFT x3]"), "一覧に [DRIFT x3] が出るべき:\n{screen}");
    assert!(screen.contains("ドリフト検知"), "詳細にドリフト警告が出るべき:\n{screen}");
}

#[test]
fn tui_togaki_modal_resolves_parent_and_renders_input_generating_and_preview_states() {
    use daihon::model::Priority;
    use daihon::register::{OverlapFinding, PriorityProposal, RegistrationProposal};
    use daihon::tui::app::{DraftTarget, ModalState};

    let mut app = App::load(Path::new("examples/scenarios")).expect("load sample scenarios");

    // 1. Selected entry 0 is parent `rio-herdr-cross-machine`; pressing `t` targets it.
    app.open_togaki_modal();
    match &app.modal {
        ModalState::Input {
            target: DraftTarget::Togaki { parent_id, .. },
            ..
        } => assert_eq!(parent_id, "rio-herdr-cross-machine"),
        other => panic!("expected Input modal for parent, got {other:?}"),
    }

    // Render input modal with Japanese text.
    if let ModalState::Input { input, .. } = &mut app.modal {
        input.push_str("遅延が 50ms 以内に収まること");
    }
    let mut terminal = Terminal::new(TestBackend::new(120, 32)).unwrap();
    terminal.draw(|f| render(f, &app)).unwrap();
    let input_screen = buffer_to_string(&terminal);
    assert!(input_screen.contains("ト書き（Togaki）を追加"));
    assert!(input_screen.contains("遅延が 50ms 以内に収まること"));

    // 2. Selecting child Togaki (entry 1) and pressing `t` resolves back to its parent scenario!
    app.close_modal(None);
    app.next();
    assert_eq!(
        app.selected_scenario().unwrap().scenario.front_matter.id,
        "rio-herdr-reconnect-latency"
    );
    app.open_togaki_modal();
    match &app.modal {
        ModalState::Input {
            target: DraftTarget::Togaki { parent_id, .. },
            ..
        } => assert_eq!(parent_id, "rio-herdr-cross-machine"),
        other => panic!("expected sibling Togaki to attach to same parent, got {other:?}"),
    }

    // 3. Render Generating spinner modal.
    app.modal = ModalState::Generating {
        target: DraftTarget::Togaki {
            parent_id: "rio-herdr-cross-machine".to_string(),
            parent_title: "Rio から herdr".to_string(),
        },
        input: "遅延が 50ms 以内に収まること".to_string(),
        tick: 2,
    };
    terminal.clear().unwrap();
    terminal.draw(|f| render(f, &app)).unwrap();
    let gen_screen = buffer_to_string(&terminal);
    assert!(gen_screen.contains("ト書き生成中"));

    // 4. Render Preview confirmation modal.
    let sample_sc = app.selected_scenario().unwrap().scenario.clone();
    app.modal = ModalState::Preview {
        target: DraftTarget::Togaki {
            parent_id: "rio-herdr-cross-machine".to_string(),
            parent_title: "Rio から herdr".to_string(),
        },
        proposal: Box::new(RegistrationProposal {
            scenario: sample_sc,
            overlaps: vec![OverlapFinding {
                existing_id: "rio-herdr-reconnect-latency".to_string(),
                existing_title: "既存ト書き".to_string(),
                reason: "遅延チェックが重複".to_string(),
            }],
            priority_proposal: PriorityProposal {
                suggested: Priority::P1,
                rationale: "品質に直結するため".to_string(),
            },
        }),
        priority: Priority::P0,
        scroll: 0,
    };
    terminal.clear().unwrap();
    terminal.draw(|f| render(f, &app)).unwrap();
    let preview_screen = buffer_to_string(&terminal);
    assert!(
        preview_screen.contains("ト書きプレビュー確認"),
        "Preview title missing:\n{preview_screen}"
    );
    assert!(
        preview_screen.contains("重複チェック"),
        "Overlap missing:\n{preview_screen}"
    );
    assert!(
        preview_screen.contains("[y/Enter] 保存"),
        "Action bar missing:\n{preview_screen}"
    );
}
