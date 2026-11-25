use std::fs;
use std::path::Path;

use daihon::model::{
    CheckState, ScenarioStatus, StepStatus, update_scenario_markdown, validate_file, validate_path,
    validate_str,
};
use tempfile::TempDir;

#[test]
fn sample_scenarios_directory_passes_validation() {
    let report = validate_path(Path::new("examples/scenarios"));
    assert!(
        report.is_ok(),
        "見本シナリオが形式チェックを通るべきです: {report:#?}"
    );
    assert_eq!(report.files.len(), 3);
}

#[test]
fn rio_herdr_sample_parses_expected_fields() {
    let scenario = validate_file(Path::new(
        "examples/scenarios/01-rio-herdr-cross-machine.md",
    ))
    .expect("Rio/herdr 見本シナリオの検証に成功するべきです");

    assert_eq!(scenario.front_matter.id, "rio-herdr-cross-machine");
    assert_eq!(scenario.front_matter.status, ScenarioStatus::Failed);
    assert_eq!(scenario.steps.len(), 3);
    assert_eq!(scenario.steps[0].status, StepStatus::ReadyForManual);
    assert_eq!(scenario.steps[1].status, StepStatus::Failed);
    assert_eq!(scenario.steps[1].failed_check.as_deref(), Some("s2-c2"));
    assert_eq!(scenario.steps[2].status, StepStatus::Pending);
}

#[test]
fn broken_scenarios_fail_validation() {
    let broken_cases: &[(&str, &str)] = &[
        (
            "front matter がない",
            "## 概要\n本文\n\n## ステップ\n\n### Step 1: A\n- status: pending\n\n- [ ] [auto] s1-c1: ok\n",
        ),
        (
            "id が kebab-case でない",
            "---\nid: Bad_ID\ntitle: テスト\npriority: P0\nstatus: pending\n---\n\n## 概要\n説明\n\n## ステップ\n\n### Step 1: A\n- status: pending\n\n- [ ] [auto] s1-c1: ok\n",
        ),
        (
            "priority が不正",
            "---\nid: valid-id\ntitle: テスト\npriority: HIGH\nstatus: pending\n---\n\n## 概要\n説明\n\n## ステップ\n\n### Step 1: A\n- status: pending\n\n- [ ] [auto] s1-c1: ok\n",
        ),
        (
            "概要が空",
            "---\nid: valid-id\ntitle: テスト\npriority: P0\nstatus: pending\n---\n\n## 概要\n\n## ステップ\n\n### Step 1: A\n- status: pending\n\n- [ ] [auto] s1-c1: ok\n",
        ),
        (
            "ステップ番号が連番でない",
            "---\nid: valid-id\ntitle: テスト\npriority: P0\nstatus: pending\n---\n\n## 概要\n説明\n\n## ステップ\n\n### Step 2: A\n- status: pending\n\n- [ ] [auto] s2-c1: ok\n",
        ),
        (
            "チェック項目に検証区分がない",
            "---\nid: valid-id\ntitle: テスト\npriority: P0\nstatus: pending\n---\n\n## 概要\n説明\n\n## ステップ\n\n### Step 1: A\n- status: pending\n\n- [ ] s1-c1: ok\n",
        ),
        (
            "チェック ID が重複している",
            "---\nid: valid-id\ntitle: テスト\npriority: P0\nstatus: pending\n---\n\n## 概要\n説明\n\n## ステップ\n\n### Step 1: A\n- status: pending\n\n- [ ] [auto] dup-1: ok\n- [ ] [auto] dup-1: ok2\n",
        ),
        (
            "failed なのに failed_check が欠落",
            "---\nid: valid-id\ntitle: テスト\npriority: P0\nstatus: failed\n---\n\n## 概要\n説明\n\n## ステップ\n\n### Step 1: A\n- status: failed\n\n- [!] [auto] s1-c1: ng\n",
        ),
        (
            "チェックは未通過なのに passed と矛盾",
            "---\nid: valid-id\ntitle: テスト\npriority: P0\nstatus: passed\n---\n\n## 概要\n説明\n\n## ステップ\n\n### Step 1: A\n- status: passed\n\n- [ ] [auto] s1-c1: not yet\n",
        ),
        (
            "front matter の status がステップと矛盾",
            "---\nid: valid-id\ntitle: テスト\npriority: P0\nstatus: passed\n---\n\n## 概要\n説明\n\n## ステップ\n\n### Step 1: A\n- status: pending\n\n- [ ] [auto] s1-c1: not yet\n",
        ),
        (
            "last_checked_at が RFC3339 ではない",
            "---\nid: valid-id\ntitle: テスト\npriority: P0\nstatus: pending\nlast_checked_at: 2026/11/25\n---\n\n## 概要\n説明\n\n## ステップ\n\n### Step 1: A\n- status: pending\n\n- [ ] [auto] s1-c1: ok\n",
        ),
    ];

    for (name, content) in broken_cases {
        let result = validate_str(content, None);
        assert!(
            result.is_err(),
            "壊れたケース '{name}' はエラーになるべきですが通ってしまいました"
        );
    }
}

#[test]
fn duplicate_scenario_ids_across_files_are_rejected() {
    let dir = TempDir::new().expect("tempdir");
    let valid = "\
---
id: same-id
title: 重複テスト
priority: P1
status: pending
---

## 概要
重複検出のテスト。

## ステップ

### Step 1: 最初
- status: pending

- [ ] [auto] s1-c1: 何かできる
";
    fs::write(dir.path().join("01-a.md"), valid).unwrap();
    fs::write(dir.path().join("02-b.md"), valid).unwrap();

    let report = validate_path(dir.path());
    assert!(!report.is_ok());
    assert_eq!(report.cross_file_issues.len(), 1);
    assert!(report.cross_file_issues[0].message.contains("same-id"));
}

#[test]
fn surgical_markdown_update_preserves_overview_and_extra_metadata() {
    let original = "\
---
id: preserve-test
title: 書き戻し保護テスト
priority: P0
status: pending
last_checked_at: null
last_checked_by: null
notified_pass_at: null
custom_owner: alice
tags:
  - core
---

## 概要
ここは自然言語の本文です。
```markdown
## 疑似見出し（コードブロック内）
```
絶対に変更されてはいけません。

## ステップ

### Step 1: 自動と手動を含むステップ
- status: pending

- [ ] [auto] s1-c1: コマンドが成功する
- [ ] [manual] s1-c2: 実機で問題なく見える

## メモ
末尾の補足セクションもそのまま残ること。
";

    let mut scenario = validate_str(original, None).expect("初期検証が通るべき");
    scenario.steps[0].checks[0].state = CheckState::Passed;
    scenario.recompute_statuses();
    scenario.front_matter.last_checked_at = Some("2026-11-25T10:00:00+09:00".to_string());
    scenario.front_matter.last_checked_by = Some("codex".to_string());

    let updated = update_scenario_markdown(&scenario);

    // 1. Re-validating the updated file must succeed.
    let reparsed = validate_str(&updated, None).expect("書き戻し後も形式チェックが通るべき");
    assert_eq!(reparsed.front_matter.status, ScenarioStatus::ReadyForManual);
    assert_eq!(reparsed.steps[0].status, StepStatus::ReadyForManual);

    // 2. Natural language overview, extra front matter key, and trailing section stay intact.
    assert!(updated.contains("custom_owner: alice"));
    assert!(updated.contains("ここは自然言語の本文です。\n```markdown\n## 疑似見出し（コードブロック内）\n```\n絶対に変更されてはいけません。"));
    assert!(updated.contains("## メモ\n末尾の補足セクションもそのまま残ること。"));
    assert!(updated.contains("status: ready_for_manual"));
    assert!(updated.contains("last_checked_by: codex"));
}

#[test]
fn cli_validate_exits_zero_for_samples_and_nonzero_for_broken_file() {
    use std::process::Command;

    let bin = env!("CARGO_BIN_EXE_daihon");

    let ok_out = Command::new(bin)
        .args(["validate", "examples/scenarios"])
        .output()
        .expect("run daihon validate");
    assert!(
        ok_out.status.success(),
        "見本ディレクトリでは終了コード 0 になるべき: {}",
        String::from_utf8_lossy(&ok_out.stderr)
    );

    let dir = TempDir::new().expect("tempdir");
    let broken_path = dir.path().join("broken.md");
    fs::write(&broken_path, "---\nid: bad\n---\n").unwrap();

    let ng_out = Command::new(bin)
        .arg("validate")
        .arg(&broken_path)
        .output()
        .expect("run daihon validate on broken file");
    assert!(
        !ng_out.status.success(),
        "壊れたシナリオでは非ゼロで終了するべき"
    );
}
