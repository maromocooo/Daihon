use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use daihon::config::{DriftConfig, EnvironmentConfig, HarnessConfig};
use daihon::model::{ScenarioStatus, StepStatus, validate_file};
use daihon::notify::NoopNotifier;
use daihon::runner::mock::MockRunner;
use daihon::runner::{AgentKind, AgentResponse};
use daihon::tui::app::App;
use daihon::tui::render::render;
use daihon::watcher::trigger::{RunLock, install_post_commit_hook, parse_interval_secs};
use daihon::watcher::{WatchOptions, resolve_distinct_watcher_agent, watch_scenario};
use serde_json::json;
use tempfile::TempDir;

/// Helper that builds a realistic command-executing mock watcher which actually runs
/// `sample-cli` inside the temporary testbed project to evaluate each step!
fn build_sample_cli_watcher(cli_path: PathBuf, state_home: PathBuf) -> MockRunner {
    MockRunner::from_fn("codex-watcher", move |req| {
        let run_cli = |args: &[&str]| -> (bool, String) {
            let out = Command::new(&cli_path)
                .args(args)
                .env("SAMPLE_CLI_HOME", &state_home)
                .output()
                .expect("execute sample-cli");
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
            let ok2 = ok2 && out2.contains("demo");
            json!({
                "checks": [
                    {
                        "check_id": "s1-c1",
                        "passed": ok1,
                        "command_run": "sample-cli init demo",
                        "evidence": out1,
                        "reason": if ok1 { json!(null) } else { json!(out1) }
                    },
                    {
                        "check_id": "s1-c2",
                        "passed": ok2,
                        "command_run": "sample-cli list",
                        "evidence": out2,
                        "reason": if ok2 { json!(null) } else { json!("demo not in list") }
                    }
                ]
            })
        } else if req.prompt.contains("Step 2:") {
            let (ok1, out1) = run_cli(&["handoff", "demo", "--target", "ubuntu"]);
            let (ok2, out2) = run_cli(&["status", "demo"]);
            let ok2 = ok1 && ok2 && out2.contains("target: ubuntu");
            json!({
                "checks": [
                    {
                        "check_id": "s2-c1",
                        "passed": ok1,
                        "command_run": "sample-cli handoff demo --target ubuntu",
                        "evidence": out1,
                        "reason": if ok1 { json!(null) } else { json!(format!("handoff 未実装: {out1}")) }
                    },
                    {
                        "check_id": "s2-c2",
                        "passed": ok2,
                        "command_run": "sample-cli status demo",
                        "evidence": out2,
                        "reason": if ok2 { json!(null) } else { json!("target: ubuntu が表示されない") }
                    }
                ]
            })
        } else {
            let (ok1, out1) = run_cli(&["resume", "demo"]);
            let ok1 = ok1 && out1.contains("ubuntu");
            json!({
                "checks": [
                    {
                        "check_id": "s3-c1",
                        "passed": ok1,
                        "command_run": "sample-cli resume demo",
                        "evidence": out1,
                        "reason": if ok1 { json!(null) } else { json!(out1) }
                    }
                ]
            })
        };

        Ok(AgentResponse {
            agent_name: "codex-watcher".to_string(),
            raw_stdout: body.to_string(),
            raw_stderr: String::new(),
            structured_json: Some(body),
        })
    })
}

fn patch_sample_cli_with_handoff(cli_path: &Path) {
    let src = fs::read_to_string(cli_path).unwrap();
    let handoff_fn = r#"
def cmd_handoff(args: list[str]) -> int:
    if len(args) != 3 or args[1] != "--target":
        print("usage: sample-cli handoff <name> --target <host>", file=sys.stderr)
        return 2
    name, target = args[0], args[2]
    path = session_file(name)
    if not path.exists():
        print(f"session not found: {name}", file=sys.stderr)
        return 1
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
    fs::write(cli_path, patched).unwrap();
}

#[test]
fn watcher_stops_at_missing_feature_then_passes_all_steps_after_fix_and_reflects_in_tui() {
    let dir = TempDir::new().unwrap();
    let scenarios_dir = dir.path().join("scenarios");
    let state_dir = dir.path().join(".daihon");
    fs::create_dir_all(&scenarios_dir).unwrap();

    // Copy sample-cli and sample scenario into isolated tempdir.
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

    let watcher = build_sample_cli_watcher(cli_path.clone(), dir.path().join("cli-state"));
    let opts = WatchOptions {
        working_dir: dir.path().to_path_buf(),
        state_dir: state_dir.clone(),
        timeout_secs: 60,
        sandbox: Some("workspace-write".to_string()),
        environment: EnvironmentConfig::default(),
        drift: DriftConfig::default(),
        parent_context: None,
        notify_topic: "test".to_string(),
        notify_on_ready_for_manual: true,
    };

    // 1. Initial run against incomplete sample-cli: Step 1 passes, Step 2 stops at s2-c1, Step 3 stays pending.
    let report1 = watch_scenario(&scenario_path, &watcher, &NoopNotifier, &opts)
        .expect("watch run 1 should complete");
    assert_eq!(report1.new_status, ScenarioStatus::Failed);
    assert_eq!(report1.stopped_step, Some(2));
    assert_eq!(report1.stopped_check.as_deref(), Some("s2-c1"));

    let updated1 = validate_file(&scenario_path).expect("updated scenario must stay valid");
    assert_eq!(updated1.steps[0].status, StepStatus::Passed);
    assert_eq!(updated1.steps[1].status, StepStatus::Failed);
    assert_eq!(updated1.steps[2].status, StepStatus::Pending);

    // Check Stage 2 TUI reflects the failure cleanly.
    let app = App::load(&scenarios_dir).unwrap();
    let mut terminal = Terminal::new(TestBackend::new(120, 28)).unwrap();
    terminal.draw(|f| render(f, &app)).unwrap();
    let rendered = terminal.backend().to_string();
    assert!(rendered.contains("[FAIL]"));
    assert!(rendered.contains("handoff 未実装"));

    // 2. Add the missing `handoff` feature to sample-cli, then re-run watcher -> all steps pass!
    patch_sample_cli_with_handoff(&cli_path);

    let report2 = watch_scenario(&scenario_path, &watcher, &NoopNotifier, &opts)
        .expect("watch run 2 should complete");
    assert_eq!(report2.new_status, ScenarioStatus::Passed);
    assert_eq!(report2.stopped_step, None);

    let updated2 = validate_file(&scenario_path).expect("all-passed scenario must be valid");
    assert_eq!(updated2.front_matter.status, ScenarioStatus::Passed);
    assert!(updated2.steps.iter().all(|s| s.status == StepStatus::Passed));
}

#[test]
fn watcher_enforces_distinct_agent_and_supports_hooks_and_locks() {
    let mut cfg = HarnessConfig::default();
    cfg.agents.developer = AgentKind::Claude;
    cfg.agents.watcher = AgentKind::Claude;
    cfg.agents.allow_same_agent = false;

    assert!(
        resolve_distinct_watcher_agent(&cfg, None, false).is_err(),
        "開発役と見張り役が同じエージェントのときは既定で拒否されるべき"
    );
    assert_eq!(
        resolve_distinct_watcher_agent(&cfg, Some(AgentKind::Codex), false).unwrap(),
        AgentKind::Codex
    );
    assert_eq!(
        resolve_distinct_watcher_agent(&cfg, None, true).unwrap(),
        AgentKind::Claude
    );

    // Lockfile prevents overlapping runs and cleans up on Drop.
    let dir = TempDir::new().unwrap();
    {
        let _lock1 = RunLock::try_acquire(dir.path()).unwrap().expect("first lock");
        assert!(RunLock::try_acquire(dir.path()).unwrap().is_none());
    }
    assert!(RunLock::try_acquire(dir.path()).unwrap().is_some());

    // Interval parsing.
    assert_eq!(parse_interval_secs("30s").unwrap(), 30);
    assert_eq!(parse_interval_secs("15m").unwrap(), 900);
    assert_eq!(parse_interval_secs("1h").unwrap(), 3600);

    // Git post-commit hook installation is idempotent.
    fs::create_dir_all(dir.path().join(".git/hooks")).unwrap();
    let hook1 = install_post_commit_hook(dir.path(), "daihon").unwrap();
    let content1 = fs::read_to_string(&hook1).unwrap();
    install_post_commit_hook(dir.path(), "daihon").unwrap();
    let content2 = fs::read_to_string(&hook1).unwrap();
    assert_eq!(content1, content2);
    assert!(content2.contains("daihon watch --background"));
}
