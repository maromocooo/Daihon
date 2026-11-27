use std::fs;
use std::path::PathBuf;
use std::process::Command;

use daihon::config::{DriftConfig, EnvironmentConfig};
use daihon::dev_loop::{DevLoopOptions, LoopOutcome, run_dev_loop};
use daihon::model::{ScenarioStatus, validate_file};
use daihon::notify::DryRunNotifier;
use daihon::runner::AgentResponse;
use daihon::runner::mock::MockRunner;
use serde_json::json;
use tempfile::TempDir;

fn build_real_sample_watcher(cli_path: PathBuf, state_home: PathBuf) -> MockRunner {
    MockRunner::from_fn("codex", move |req| {
        let run_cli = |args: &[&str]| -> (bool, String) {
            let out = Command::new(&cli_path)
                .args(args)
                .env("SAMPLE_CLI_HOME", &state_home)
                .output()
                .expect("run sample-cli");
            let combined = format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            (out.status.success(), combined.trim().to_string())
        };

        let body = if req.prompt.contains("Step 1:") {
            let (ok1, out1) = run_cli(&["init", "demo"]);
            let (ok2, out2) = run_cli(&["list"]);
            json!({
                "checks": [
                    { "check_id": "s1-c1", "passed": ok1, "command_run": "sample-cli init demo", "evidence": out1, "reason": null },
                    { "check_id": "s1-c2", "passed": ok2 && out2.contains("demo"), "command_run": "sample-cli list", "evidence": out2, "reason": null }
                ]
            })
        } else if req.prompt.contains("Step 2:") {
            let (ok1, out1) = run_cli(&["handoff", "demo", "--target", "ubuntu"]);
            let (ok2, out2) = run_cli(&["status", "demo"]);
            let ok2 = ok1 && ok2 && out2.contains("target: ubuntu");
            json!({
                "checks": [
                    { "check_id": "s2-c1", "passed": ok1, "command_run": "sample-cli handoff demo --target ubuntu", "evidence": out1, "reason": if ok1 { json!(null) } else { json!("handoff が未実装") } },
                    { "check_id": "s2-c2", "passed": ok2, "command_run": "sample-cli status demo", "evidence": out2, "reason": if ok2 { json!(null) } else { json!("target: ubuntu がない") } }
                ]
            })
        } else {
            let (ok1, out1) = run_cli(&["resume", "demo"]);
            json!({
                "checks": [
                    { "check_id": "s3-c1", "passed": ok1 && out1.contains("ubuntu"), "command_run": "sample-cli resume demo", "evidence": out1, "reason": null }
                ]
            })
        };

        Ok(AgentResponse {
            agent_name: "codex".to_string(),
            raw_stdout: body.to_string(),
            raw_stderr: String::new(),
            structured_json: Some(body),
        })
    })
}

#[test]
fn dev_loop_respects_approval_gate_then_implements_missing_feature_and_re_verifies() {
    let dir = TempDir::new().unwrap();
    let scenarios_dir = dir.path().join("scenarios");
    let state_dir = dir.path().join(".daihon");
    fs::create_dir_all(&scenarios_dir).unwrap();

    let cli_path = dir.path().join("sample-cli");
    fs::copy("examples/sample-project/sample-cli", &cli_path).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&cli_path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    let scenario_path = scenarios_dir.join("02-sample-project-workflow.md");
    fs::copy(
        "examples/scenarios/02-sample-project-workflow.md",
        &scenario_path,
    )
    .unwrap();

    let watcher = build_real_sample_watcher(cli_path.clone(), dir.path().join("cli-state"));
    let cli_for_dev = cli_path.clone();
    let developer = MockRunner::from_fn("claude", move |_req| {
        // Developer agent receives the missing-feature task and implements `handoff`.
        let src = fs::read_to_string(&cli_for_dev).unwrap();
        let handoff_fn = r#"
def cmd_handoff(args: list[str]) -> int:
    name, target = args[0], args[2]
    path = session_file(name)
    data = json.loads(path.read_text(encoding="utf-8"))
    data["target"] = target
    data["status"] = "handed_off"
    path.write_text(json.dumps(data, indent=2), encoding="utf-8")
    print(f"handed off {name} -> {target}")
    return 0
"#;
        let patched = src
            .replace("def main() -> int:", &format!("{handoff_fn}\ndef main() -> int:"))
            .replace(
                "    if sub == \"status\":",
                "    if sub == \"handoff\":\n        return cmd_handoff(rest)\n    if sub == \"status\":",
            );
        fs::write(&cli_for_dev, patched).unwrap();

        Ok(AgentResponse {
            agent_name: "claude".to_string(),
            raw_stdout: "implemented handoff subcommand".to_string(),
            raw_stderr: String::new(),
            structured_json: None,
        })
    });

    let notifier = DryRunNotifier::new(state_dir.join("notifications.jsonl"));

    // 1. First run: check_first detects failure, generates task, user DECLINES approval.
    let opts_declined = DevLoopOptions {
        working_dir: dir.path().to_path_buf(),
        scenarios_dir: scenarios_dir.clone(),
        state_dir: state_dir.clone(),
        target_scenario_id: Some("sample-project-workflow".to_string()),
        check_first: true,
        plan_only: false,
        auto_approve: false,
        max_retries: 2,
        allow_same_agent: false,
        watcher_sandbox: Some("workspace-write".to_string()),
        environment: EnvironmentConfig::default(),
        drift: DriftConfig::default(),
        timeout_secs: 60,
        developer_timeout_secs: 900,
        notify_topic: "test-topic".to_string(),
        notify_on_ready_for_manual: true,
    };

    let outcome1 = run_dev_loop(&developer, &watcher, &notifier, &opts_declined, |task| {
        assert_eq!(task.stopped_step_number, 2);
        assert_eq!(task.failed_check_id, "s2-c1");
        assert!(task.task_file.exists());
        Ok(false) // user declines
    })
    .expect("declined loop should succeed cleanly");

    assert!(matches!(outcome1, LoopOutcome::DeclinedByUser { .. }));
    assert_eq!(
        developer.recorded_calls().len(),
        0,
        "未承認のときは開発役が一切呼ばれないこと"
    );
    assert!(
        notifier.read_records().unwrap().is_empty(),
        "失敗段階では通知されないこと"
    );

    // 2. Second run: user APPROVES -> developer implements handoff -> watcher re-verifies -> all steps pass & notifies!
    let opts_approved = DevLoopOptions {
        check_first: false,
        ..opts_declined
    };

    let outcome2 = run_dev_loop(&developer, &watcher, &notifier, &opts_approved, |_task| {
        Ok(true)
    })
    .expect("approved loop should resolve");

    match outcome2 {
        LoopOutcome::Resolved {
            scenario_id,
            final_status,
            iterations,
            last_watch,
            ..
        } => {
            assert_eq!(scenario_id, "sample-project-workflow");
            assert_eq!(final_status, ScenarioStatus::Passed);
            assert_eq!(iterations, 1);
            assert!(last_watch.notified);
        }
        other => panic!("期待: Resolved, 実際: {other:?}"),
    }

    assert_eq!(developer.recorded_calls().len(), 1);
    assert!(
        developer.recorded_calls()[0]
            .prompt
            .contains("Step 2")
    );
    let final_sc = validate_file(&scenario_path).unwrap();
    assert_eq!(final_sc.front_matter.status, ScenarioStatus::Passed);

    let notifications = notifier.read_records().unwrap();
    assert_eq!(
        notifications.len(),
        1,
        "全ステップ通過した瞬間にだけ 1 通通知が記録されること"
    );
    assert_eq!(notifications[0].scenario_id, "sample-project-workflow");
}
