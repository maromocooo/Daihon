use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use daihon::integrations::{
    InstallOptions, IntegrationTarget, install_integration, install_integration_with_options,
};
use daihon::runner::agy::AgyRunner;
use daihon::runner::claude::ClaudeRunner;
use daihon::runner::codex::CodexRunner;
use daihon::runner::{AgentRequest, ExecutionMode, extract_json_value};
use daihon::watcher::trigger::{HookEventKind, handle_hook_event};
use serde_json::{Value, json};
use tempfile::TempDir;

#[test]
fn install_integrations_merges_rules_and_hooks_safely_and_idempotently() {
    // 1. agy: preserves existing AGENTS.md and existing .agents/hooks.json entries!
    let agy_dir = TempDir::new().unwrap();
    fs::write(agy_dir.path().join("AGENTS.md"), "# 既存プロジェクトルール\n- そのまま残ること\n")
        .unwrap();
    fs::create_dir_all(agy_dir.path().join(".agents")).unwrap();
    fs::write(
        agy_dir.path().join(".agents/hooks.json"),
        r#"{"existing-lint": {"PostToolUse": []}}"#,
    )
    .unwrap();

    let agy_files = install_integration(IntegrationTarget::Agy, agy_dir.path()).unwrap();
    assert_eq!(agy_files.len(), 3); // SKILL.md + AGENTS.md + .agents/hooks.json

    let agents_md = fs::read_to_string(agy_dir.path().join("AGENTS.md")).unwrap();
    assert!(agents_md.contains("# 既存プロジェクトルール"));
    assert!(agents_md.contains("<!-- daihon:start -->"));
    assert!(agents_md.contains("NEXT_TASK.md"));

    // Re-running install must be idempotent (only 1 managed block).
    install_integration(IntegrationTarget::Agy, agy_dir.path()).unwrap();
    let agents_md_again = fs::read_to_string(agy_dir.path().join("AGENTS.md")).unwrap();
    assert_eq!(
        agents_md_again.matches("<!-- daihon:start -->").count(),
        1
    );

    let hooks_json: Value =
        serde_json::from_str(&fs::read_to_string(agy_dir.path().join(".agents/hooks.json")).unwrap())
            .unwrap();
    assert!(hooks_json.get("existing-lint").is_some());
    assert!(hooks_json["daihon"]["SessionStart"].is_array());
    assert!(hooks_json["daihon"]["Stop"].is_array());

    // 2. codex: installs skill + AGENTS.md rule (respecting --no-rules too).
    let codex_dir = TempDir::new().unwrap();
    let codex_files = install_integration_with_options(
        IntegrationTarget::Codex,
        codex_dir.path(),
        InstallOptions {
            include_rules: false,
            include_hooks: false,
        },
    )
    .unwrap();
    assert_eq!(codex_files.len(), 1);
    assert!(!codex_dir.path().join("AGENTS.md").exists());

    // 3. claude: preserves existing CLAUDE.md and .claude/settings.json keys & hooks!
    let claude_dir = TempDir::new().unwrap();
    fs::write(claude_dir.path().join("CLAUDE.md"), "# 既存 CLAUDE ルール\n").unwrap();
    fs::create_dir_all(claude_dir.path().join(".claude")).unwrap();
    fs::write(
        claude_dir.path().join(".claude/settings.json"),
        r#"{"model":"opus","hooks":{"Stop":[{"hooks":[{"type":"command","command":"echo existing"}]}]}}"#,
    )
    .unwrap();

    let claude_files = install_integration(IntegrationTarget::Claude, claude_dir.path()).unwrap();
    assert_eq!(claude_files.len(), 7); // skill + 4 commands + CLAUDE.md + settings.json

    let claude_md = fs::read_to_string(claude_dir.path().join("CLAUDE.md")).unwrap();
    assert!(claude_md.contains("# 既存 CLAUDE ルール"));
    assert!(claude_md.contains("<!-- daihon:start -->"));

    install_integration(IntegrationTarget::Claude, claude_dir.path()).unwrap();
    let settings: Value = serde_json::from_str(
        &fs::read_to_string(claude_dir.path().join(".claude/settings.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(settings["model"], "opus");
    assert_eq!(settings["hooks"]["Stop"].as_array().unwrap().len(), 2);
    assert_eq!(
        settings["hooks"]["SessionStart"].as_array().unwrap().len(),
        1
    );
}

#[test]
fn hook_events_follow_agy_and_claude_contracts_and_resolve_cwd_from_stdin() {
    let dir = TempDir::new().unwrap();
    let scenarios_dir = dir.path().join("scenarios");
    fs::create_dir_all(&scenarios_dir).unwrap();
    fs::copy(
        "examples/scenarios/01-rio-herdr-cross-machine.md",
        scenarios_dir.join("01-rio-herdr-cross-machine.md"),
    )
    .unwrap();

    // Simulate Antigravity running hook with process CWD = `<repo>/.agents/` and real repo in stdin `cwd`.
    let agents_subdir = dir.path().join(".agents");
    fs::create_dir_all(&agents_subdir).unwrap();
    let stdin_payload = json!({
        "conversationId": "conv-1",
        "cwd": dir.path().to_string_lossy(),
        "hookEventName": "SessionStart"
    })
    .to_string();

    let session_out = handle_hook_event(
        HookEventKind::AgySessionStart,
        &stdin_payload,
        &agents_subdir,
        &PathBuf::from("daihon"),
        false,
    )
    .unwrap();
    let session_json: Value = serde_json::from_str(&session_out).unwrap();
    let injected = session_json["injectSteps"][0]["systemMessage"]["systemMessage"]
        .as_str()
        .expect("systemMessage string");
    assert!(injected.contains("rio-herdr-cross-machine"));
    assert!(injected.contains("Step 2"));

    // Stop hook always returns {"decision":"allow"} so it never traps the user.
    let stop_stdin = json!({
        "cwd": dir.path().to_string_lossy(),
        "continuationActive": false
    })
    .to_string();
    let stop_out = handle_hook_event(
        HookEventKind::AgyStop,
        &stop_stdin,
        &agents_subdir,
        &PathBuf::from("daihon"),
        false,
    )
    .unwrap();
    let stop_json: Value = serde_json::from_str(&stop_out).unwrap();
    assert_eq!(stop_json["decision"], "allow");

    // Claude SessionStart outputs plain text context.
    let claude_out = handle_hook_event(
        HookEventKind::ClaudeSessionStart,
        "",
        dir.path(),
        &PathBuf::from("daihon"),
        false,
    )
    .unwrap();
    assert!(claude_out.contains("rio-herdr-cross-machine"));
}

#[test]
fn runner_adapters_build_expected_cli_flags_and_unwrap_envelopes() {
    let req = AgentRequest {
        working_dir: PathBuf::from("/tmp/work"),
        prompt: "verify step".to_string(),
        json_schema: Some(json!({"type": "object"})),
        mode: ExecutionMode::ReadOnly,
        sandbox_override: None,
        env_vars: BTreeMap::new(),
        timeout_secs: 120,
    };

    let agy_args = AgyRunner::new("agy").build_args(&req, Some("/tmp/s.json"));
    let output_fmt_pos = agy_args
        .iter()
        .position(|a| a == "--output-format")
        .expect("--output-format present");
    let print_pos = agy_args
        .iter()
        .position(|a| a == "--print=verify step")
        .expect("--print=<prompt> present");
    assert!(
        output_fmt_pos < print_pos,
        "--output-format は --print=<prompt> より前に置く必要がある: {agy_args:?}"
    );
    assert!(agy_args.contains(&"--disable-slash-commands".to_string()));
    assert!(agy_args.contains(&"--json-schema".to_string()));
    assert!(agy_args.contains(&"--sandbox".to_string()));
    assert!(
        !agy_args.contains(&"plan".to_string()),
        "ReadOnly の構造化出力では Planning Mode (--mode plan) を使わないこと"
    );

    let codex_args =
        CodexRunner::new("codex").build_args(&req, Some("/tmp/s.json"), "/tmp/out.json");
    assert_eq!(codex_args[0], "exec");
    assert!(codex_args.contains(&"read-only".to_string()));
    assert!(codex_args.contains(&"--output-schema".to_string()));

    let verify_req = AgentRequest {
        mode: ExecutionMode::Verify,
        sandbox_override: Some("workspace-write".to_string()),
        ..req.clone()
    };
    let codex_verify_args =
        CodexRunner::new("codex").build_args(&verify_req, Some("/tmp/s.json"), "/tmp/out.json");
    assert!(codex_verify_args.contains(&"workspace-write".to_string()));

    let claude_args = ClaudeRunner::new("claude").build_args(&req).unwrap();
    assert_eq!(claude_args[0], "-p");
    assert!(claude_args.contains(&"--json-schema".to_string()));
    assert!(claude_args.contains(&"plan".to_string()));

    // Envelope unwrapping tests.
    let claude_env =
        r#"{"type":"result","structured_output":{"checks":[{"check_id":"s1-c1","passed":true}]}}"#;
    let extracted = extract_json_value(claude_env).unwrap();
    assert_eq!(extracted["checks"][0]["check_id"], "s1-c1");

    let fenced_env = r#"{"result":"```json\n{\"ok\":true}\n```"}"#;
    assert_eq!(extract_json_value(fenced_env).unwrap()["ok"], true);

    let agy_prose_env = r#"{"conversation_id":"abc","status":"ok","response":"生成結果です:\n{\"id\":\"demo\",\"title\":\"t\"}"}"#;
    assert_eq!(extract_json_value(agy_prose_env).unwrap()["id"], "demo");

    let agy_obj_env = r#"{"conversation_id":"abc","response":{"id":"demo2","title":"t2"}}"#;
    assert_eq!(extract_json_value(agy_obj_env).unwrap()["id"], "demo2");

    let str_structured_env = r#"{"structured_output":"{\"id\":\"demo3\"}"}"#;
    assert_eq!(extract_json_value(str_structured_env).unwrap()["id"], "demo3");
}

#[test]
fn config_extra_args_propagate_to_claude_codex_and_agy_runners_safely() {
    use daihon::config::HarnessConfig;
    use daihon::runner::AgentKind;

    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join(".daihon.toml"),
        r#"
[agents]
developer = "claude"
watcher = "codex"

[agents.extra_args]
claude = ["--model", "opus", "--effort", "high"]
codex = ["-m", "gpt-5-codex"]
agy = ["--model", "pro"]
"#,
    )
    .unwrap();

    let cfg = HarnessConfig::load_from_dir(dir.path()).unwrap();
    assert_eq!(
        cfg.extra_args_for(AgentKind::Claude),
        vec!["--model", "opus", "--effort", "high"]
    );
    assert_eq!(
        cfg.extra_args_for(AgentKind::Codex),
        vec!["-m", "gpt-5-codex"]
    );
    assert_eq!(cfg.extra_args_for(AgentKind::Agy), vec!["--model", "pro"]);

    let req = AgentRequest {
        working_dir: PathBuf::from("/tmp/work"),
        prompt: "implement feature".to_string(),
        json_schema: None,
        mode: ExecutionMode::WorkspaceWrite,
        sandbox_override: None,
        env_vars: BTreeMap::new(),
        timeout_secs: 180,
    };

    // Claude: default --permission-mode acceptEdits + extra_args
    let claude_args = ClaudeRunner::new("claude")
        .with_extra_args(cfg.extra_args_for(AgentKind::Claude))
        .build_args(&req)
        .unwrap();
    assert!(claude_args.contains(&"--permission-mode".to_string()));
    assert!(claude_args.contains(&"acceptEdits".to_string()));
    assert!(claude_args.contains(&"--model".to_string()));
    assert!(claude_args.contains(&"opus".to_string()));
    assert!(claude_args.contains(&"--effort".to_string()));
    assert!(claude_args.contains(&"high".to_string()));

    // Claude: explicit permission override in extra_args must not duplicate --permission-mode
    let claude_override = ClaudeRunner::new("claude")
        .with_extra_args(vec![
            "--permission-mode".to_string(),
            "bypassPermissions".to_string(),
            "--model".to_string(),
            "opus".to_string(),
        ])
        .build_args(&req)
        .unwrap();
    assert_eq!(
        claude_override
            .iter()
            .filter(|a| a.as_str() == "--permission-mode")
            .count(),
        1
    );
    assert!(claude_override.contains(&"bypassPermissions".to_string()));

    // Codex: extra_args must appear after `exec` and BEFORE trailing `-`
    let codex_args = CodexRunner::new("codex")
        .with_extra_args(cfg.extra_args_for(AgentKind::Codex))
        .build_args(&req, None, "/tmp/out.txt");
    let model_pos = codex_args.iter().position(|a| a == "-m").unwrap();
    let dash_pos = codex_args.iter().rposition(|a| a == "-").unwrap();
    assert!(model_pos > 0 && model_pos < dash_pos);

    // Agy: extra_args must appear BEFORE `--print=<prompt>`
    let agy_args = AgyRunner::new("agy")
        .with_extra_args(cfg.extra_args_for(AgentKind::Agy))
        .build_args(&req, None);
    let agy_model_pos = agy_args.iter().position(|a| a == "--model").unwrap();
    let print_pos = agy_args
        .iter()
        .position(|a| a == "--print=implement feature")
        .unwrap();
    assert!(agy_model_pos < print_pos);
}
