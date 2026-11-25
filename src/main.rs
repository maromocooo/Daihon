use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use chrono::Local;
use clap::{Parser, Subcommand};
use serde::Serialize;

use daihon::config::{DEFAULT_CONFIG_FILE, DriftConfig, HarnessConfig};
use daihon::dev_loop::{DevLoopOptions, LoopOutcome, run_dev_loop};
use daihon::integrations::{
    InstallOptions, IntegrationTarget, install_integration_with_options,
};
use daihon::model::{
    CheckState, Priority, ScenarioStatus, StepStatus, load_scenarios_from_dir, validate_path,
};
use daihon::notify::{NotificationPayload, build_notifier, resolve_topic};
use daihon::register::{propose_registration, save_registration};
use daihon::runner::{AgentKind, build_runner};
use daihon::tui::run_tui;
use daihon::watcher::drift::{
    DriftSummary, analyze_scenario_drift, compute_worktree_fingerprint, render_agent_context,
};
use daihon::watcher::trigger::{
    BackgroundWatchRequest, HookEventKind, RunLock, daemon_status, handle_hook_event,
    install_post_commit_hook, is_unchanged_since_last_watch, parse_interval_secs,
    record_last_watch_fingerprint, spawn_background_watch, stop_daemon_watch,
};
use daihon::watcher::{
    WatchOptions, resolve_distinct_watcher_agent, update_check_manually, watch_scenario,
};

#[derive(Debug, Parser)]
#[command(
    name = "daihon",
    version,
    about = "Daihon — シナリオ駆動の開発ハーネス CLI（登録・見張り・可視化・ドリフト検知・通知）"
)]
struct Cli {
    /// 作業対象リポジトリのルートディレクトリ（既定: カレントディレクトリ）
    #[arg(long, global = true)]
    workdir: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// プロジェクトに設定ファイル（.daihon.toml）と scenarios/ を初期作成する
    Init {
        /// 既存の設定ファイルを上書きする
        #[arg(long)]
        force: bool,
    },

    /// シナリオファイルの保存形式と状態整合性をチェックする（段階1）
    Validate {
        /// 検証対象のファイルまたはディレクトリ（既定: 設定の scenarios_dir）
        path: Option<PathBuf>,

        /// 結果を JSON 形式で出力する
        #[arg(long)]
        json: bool,
    },

    /// 最小の TUI でシナリオ一覧・優先度・ステップ通過状況・ドリフト警告を表示する（段階2）
    Tui {
        /// シナリオディレクトリ（既定: 設定の scenarios_dir）
        #[arg(long)]
        dir: Option<PathBuf>,
    },

    /// シナリオ一覧と通過状況・ドリフト状態を非対話テキストまたは JSON で表示する（段階2）
    Status {
        /// シナリオディレクトリ（既定: 設定の scenarios_dir）
        #[arg(long)]
        dir: Option<PathBuf>,

        /// JSON 形式で出力する
        #[arg(long)]
        json: bool,
    },

    /// 開発役エージェントへ渡すための簡潔な現在地・ドリフト要約を出力する
    Context {
        /// シナリオディレクトリ（既定: 設定の scenarios_dir）
        #[arg(long)]
        dir: Option<PathBuf>,
    },

    /// 自然言語からシナリオを構造化し、重複・優先度案を確認して登録する（段階3）
    Register {
        /// 自然言語のシナリオ説明（省略時は標準入力／対話プロンプトから読み込み）
        text: Option<String>,

        /// シナリオ保存先ディレクトリ（既定: 設定の scenarios_dir）
        #[arg(long)]
        dir: Option<PathBuf>,

        /// 登録に使用するエージェント（agy | codex | claude）
        #[arg(long)]
        agent: Option<AgentKind>,

        /// 優先度を明示指定する（P0〜P3）
        #[arg(long)]
        priority: Option<Priority>,

        /// 確認プロンプトをスキップして保存する
        #[arg(long)]
        yes: bool,

        /// ファイルに保存せずプレビューと重複・優先度案のみ表示する
        #[arg(long)]
        dry_run: bool,
    },

    /// 開発役とは別の見張り役エージェントでシナリオを検証する（段階4）
    Watch {
        /// シナリオディレクトリ（既定: 設定の scenarios_dir）
        #[arg(long)]
        dir: Option<PathBuf>,

        /// 特定のシナリオ ID のみ検証する
        #[arg(long)]
        scenario: Option<String>,

        /// 見張り役エージェントを上書き指定（agy | codex | claude）
        #[arg(long)]
        watcher: Option<AgentKind>,

        /// 開発役と同じエージェントの見張り実行を許可する
        #[arg(long)]
        allow_same_agent: bool,

        /// 通知を実送信せず `.daihon/notifications.jsonl` へ記録のみ行う
        #[arg(long)]
        dry_run_notify: bool,

        /// 前回の見張り実行からプロダクトコードに変化がない場合はスキップする
        #[arg(long)]
        if_changed: bool,

        /// 裏で非同期実行して即座にプロンプトへ戻る（--interval 併用時は定期見張りデーモン起動）
        #[arg(long)]
        background: bool,

        /// 指定間隔ごとに定期実行する（例: 30s, 15m, 1h）
        #[arg(long)]
        interval: Option<String>,

        /// 稼働中のバックグラウンド定期見張りデーモンを停止する
        #[arg(long, conflicts_with_all = ["background", "interval", "daemon_status"])]
        stop: bool,

        /// バックグラウンド定期見張りデーモンの稼働状況を確認する
        #[arg(long, conflicts_with_all = ["background", "interval", "stop"])]
        daemon_status: bool,
    },

    /// チェック項目（主に [manual]）の状態を手動で更新する
    Check {
        /// 対象のシナリオ ID
        scenario_id: String,

        /// 対象のチェック項目 ID（例: s1-c3）
        check_id: String,

        /// 通過（[x]）にする
        #[arg(long, conflicts_with_all = ["fail", "reset"])]
        pass: bool,

        /// 失敗（[!]）にする
        #[arg(long, conflicts_with_all = ["pass", "reset"])]
        fail: bool,

        /// 未検証（[ ]）に戻す
        #[arg(long, conflicts_with_all = ["pass", "fail"])]
        reset: bool,

        /// 失敗時の理由メモ
        #[arg(long)]
        reason: Option<String>,

        /// シナリオディレクトリ（既定: 設定の scenarios_dir）
        #[arg(long)]
        dir: Option<PathBuf>,

        /// 通知をドライラン記録のみにする
        #[arg(long)]
        dry_run_notify: bool,
    },

    /// git post-commit フック等を設置する（段階4）
    Hook {
        #[command(subcommand)]
        action: HookAction,
    },

    /// agy / claude のライフサイクルフックから呼ばれる高速イベントハンドラ
    HookEvent {
        /// イベント種別（agy-session-start | agy-stop | claude-session-start | claude-stop）
        event: HookEventKind,

        /// テスト用：Stop フック時にバックグラウンド見張りを起動せず判定のみ行う
        #[arg(long, hide = true)]
        no_spawn: bool,
    },

    /// 止まったステップから足りない機能を作るタスクを起票し、承認後に開発役→再見張りを行う（段階5）
    Fix {
        /// シナリオディレクトリ（既定: 設定の scenarios_dir）
        #[arg(long)]
        dir: Option<PathBuf>,

        /// 対象のシナリオ ID（省略時は最優先の失敗シナリオ）
        #[arg(long)]
        scenario: Option<String>,

        /// 着手前に見張り役を 1 回実行してから失敗ステップを探す
        #[arg(long)]
        check_first: bool,

        /// タスクファイルの起票のみ行い、開発役は起動しない
        #[arg(long)]
        plan_only: bool,

        /// 承認プロンプトをスキップして開発役へ着手させる
        #[arg(long)]
        auto_approve: bool,

        /// 最大リトライ回数（既定: 設定の loop_policy.max_retries）
        #[arg(long)]
        max_retries: Option<usize>,

        /// 開発役エージェントを上書き指定
        #[arg(long)]
        developer: Option<AgentKind>,

        /// 見張り役エージェントを上書き指定
        #[arg(long)]
        watcher: Option<AgentKind>,

        /// 開発役と見張り役が同一エージェントでも許可する
        #[arg(long)]
        allow_same_agent: bool,

        /// 通知をドライラン記録のみにする
        #[arg(long)]
        dry_run_notify: bool,
    },

    /// スマホ通知（ntfy）の疎通テストを行う（段階6）
    Notify {
        #[command(subcommand)]
        action: NotifyAction,
    },

    /// agy / codex / claude 向けの Skill・Always-On ルール・ライフサイクルフックを導入する（段階7）
    InitIntegration {
        /// 導入先のコマンド（agy | codex | claude）
        target: IntegrationTarget,

        /// 導入先プロジェクトディレクトリ（既定: workdir）
        #[arg(long)]
        project: Option<PathBuf>,

        /// AGENTS.md / CLAUDE.md への Always-On ルール追記をスキップする
        #[arg(long)]
        no_rules: bool,

        /// hooks.json / settings.json へのライフサイクルフック追記をスキップする
        #[arg(long)]
        no_hooks: bool,
    },
}

#[derive(Debug, Subcommand)]
enum HookAction {
    /// git の post-commit フックにバックグラウンド見張りを設置する
    Install {
        /// 呼び出す scenario バイナリ名またはパス
        #[arg(long, default_value = "daihon")]
        bin: String,
    },
}

#[derive(Debug, Subcommand)]
enum NotifyAction {
    /// ntfy（またはドライラン）へテスト通知を 1 通送る
    Test {
        /// トピック名を明示指定（省略時は設定または DAIHON_NTFY_TOPIC）
        #[arg(long)]
        topic: Option<String>,

        /// 送信せず `.daihon/notifications.jsonl` へ記録のみ行う
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Debug, Serialize)]
struct StatusSummaryRow {
    id: String,
    priority: Priority,
    status: ScenarioStatus,
    title: String,
    passed_steps: usize,
    total_steps: usize,
    stopped_step: Option<usize>,
    failed_check: Option<String>,
    drift: Option<DriftSummary>,
    last_checked_at: Option<String>,
    last_checked_by: Option<String>,
    path: PathBuf,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("エラー: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<ExitCode> {
    let workdir = match cli.workdir {
        Some(dir) => dir,
        None => std::env::current_dir().context("カレントディレクトリを取得できません")?,
    };

    // HookEvent resolves its own workspace from stdin JSON (`cwd` / `workspacePaths`),
    // so run it directly without failing if invoked from `.agents/`.
    if let Commands::HookEvent { event, no_spawn } = cli.command {
        let mut stdin_buf = String::new();
        if !std::io::IsTerminal::is_terminal(&io::stdin()) {
            let _ = io::stdin().read_to_string(&mut stdin_buf);
        }
        let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("daihon"));
        let out = handle_hook_event(event, &stdin_buf, &workdir, &exe, !no_spawn)?;
        if !out.is_empty() {
            println!("{out}");
        }
        return Ok(ExitCode::SUCCESS);
    }

    if !matches!(cli.command, Commands::Init { .. }) && !workdir.exists() {
        bail!(
            "作業ディレクトリが存在しません: {}（先にディレクトリを作成するか `daihon --workdir ... init` を実行してください）",
            workdir.display()
        );
    }
    let config = HarnessConfig::load_from_dir(&workdir)?;

    match cli.command {
        Commands::HookEvent { .. } => unreachable!(),

        Commands::Init { force } => {
            fs::create_dir_all(&workdir).with_context(|| {
                format!("作業ディレクトリを作成できません: {}", workdir.display())
            })?;
            let cfg_path = workdir.join(DEFAULT_CONFIG_FILE);
            if cfg_path.exists() && !force {
                println!(
                    "既存の設定ファイルを維持しました: {}（上書きは --force）",
                    cfg_path.display()
                );
            } else {
                fs::write(&cfg_path, HarnessConfig::sample_toml())?;
                println!("設定ファイルを作成しました: {}", cfg_path.display());
            }
            let sc_dir = config.resolve_scenarios_dir(&workdir, None);
            fs::create_dir_all(&sc_dir)?;
            println!("シナリオディレクトリを準備しました: {}", sc_dir.display());
            Ok(ExitCode::SUCCESS)
        }

        Commands::Validate { path, json } => {
            let target = config.resolve_scenarios_dir(&workdir, path.as_deref());
            Ok(cmd_validate(&target, json))
        }

        Commands::Tui { dir } => {
            let scenarios_dir = config.resolve_scenarios_dir(&workdir, dir.as_deref());
            let state_dir = config.resolve_state_dir(&workdir);
            run_tui(&scenarios_dir, &state_dir, config.drift.clone())?;
            Ok(ExitCode::SUCCESS)
        }

        Commands::Status { dir, json } => {
            let scenarios_dir = config.resolve_scenarios_dir(&workdir, dir.as_deref());
            let state_dir = config.resolve_state_dir(&workdir);
            cmd_status(&scenarios_dir, &state_dir, &config.drift, json)?;
            Ok(ExitCode::SUCCESS)
        }

        Commands::Context { dir } => {
            let scenarios_dir = config.resolve_scenarios_dir(&workdir, dir.as_deref());
            let state_dir = config.resolve_state_dir(&workdir);
            let ctx = render_agent_context(&scenarios_dir, &state_dir, &config.drift)?;
            if !ctx.is_empty() {
                println!("{ctx}");
            }
            Ok(ExitCode::SUCCESS)
        }

        Commands::Register {
            text,
            dir,
            agent,
            priority,
            yes,
            dry_run,
        } => {
            let scenarios_dir = config.resolve_scenarios_dir(&workdir, dir.as_deref());
            let input = read_register_input(text)?;
            let agent_kind = agent.unwrap_or(config.agents.register);
            let runner = build_runner(agent_kind, &config)?;

            println!("登録エージェント ({agent_kind}) でシナリオを構造化しています...");
            let proposal = propose_registration(
                &input,
                &scenarios_dir,
                runner.as_ref(),
                &workdir,
                config.watcher.timeout_secs,
            )?;

            println!("\n=== 生成されたシナリオ案 ===");
            println!("{}", proposal.scenario.render_new_markdown());

            println!("=== 既存シナリオとの重なり ===");
            if proposal.overlaps.is_empty() {
                println!("重なりは検出されませんでした。");
            } else {
                for ov in &proposal.overlaps {
                    println!(
                        "- {} ({}): {}",
                        ov.existing_id, ov.existing_title, ov.reason
                    );
                }
            }

            println!("\n=== 優先度案 ===");
            println!(
                "- 推奨: {} （理由: {}）",
                proposal.priority_proposal.suggested, proposal.priority_proposal.rationale
            );

            if dry_run {
                println!("\n(--dry-run のため保存せず終了します)");
                return Ok(ExitCode::SUCCESS);
            }

            let chosen_priority = if let Some(p) = priority {
                Some(p)
            } else if yes {
                None
            } else {
                match prompt_registration_confirmation(proposal.priority_proposal.suggested)? {
                    ConfirmAction::Save(p) => p,
                    ConfirmAction::Cancel => {
                        println!("保存をキャンセルしました。");
                        return Ok(ExitCode::SUCCESS);
                    }
                }
            };

            let saved = save_registration(&proposal, &scenarios_dir, chosen_priority)?;
            println!("シナリオを保存しました: {}", saved.display());
            Ok(ExitCode::SUCCESS)
        }

        Commands::Watch {
            dir,
            scenario,
            watcher,
            allow_same_agent,
            dry_run_notify,
            if_changed,
            background,
            interval,
            stop,
            daemon_status: show_daemon_status,
        } => {
            let scenarios_dir = config.resolve_scenarios_dir(&workdir, dir.as_deref());
            let state_dir = config.resolve_state_dir(&workdir);

            if stop {
                match stop_daemon_watch(&state_dir)? {
                    Some(pid) => println!("定期見張りデーモンを停止しました (pid: {pid})"),
                    None => println!("稼働中の定期見張りデーモンはありません。"),
                }
                return Ok(ExitCode::SUCCESS);
            }

            if show_daemon_status {
                let st = daemon_status(&state_dir);
                if st.running {
                    println!(
                        "定期見張りデーモン: 稼働中 (pid: {}, log: {})",
                        st.pid.unwrap_or_default(),
                        st.log_path.display()
                    );
                } else {
                    println!("定期見張りデーモン: 停止中");
                }
                return Ok(ExitCode::SUCCESS);
            }

            if background {
                if let Some(ref iv) = interval {
                    // Validate interval format before spawning background daemon.
                    parse_interval_secs(iv)?;
                }
                let exe = std::env::current_exe()?;
                let pid = spawn_background_watch(&BackgroundWatchRequest {
                    current_exe: &exe,
                    scenarios_dir: &scenarios_dir,
                    scenario_id: scenario.as_deref(),
                    state_dir: &state_dir,
                    working_dir: &workdir,
                    dry_run_notify,
                    if_changed,
                    interval: interval.as_deref(),
                })?;
                if let Some(ref iv) = interval {
                    println!(
                        "定期見張りデーモンを裏で起動しました (interval: {iv}, pid: {pid}, log: {})",
                        state_dir.join("watch-daemon.log").display()
                    );
                    println!("状態確認: `daihon watch --daemon-status` / 停止: `daihon watch --stop`");
                } else {
                    println!(
                        "バックグラウンドで見張り役を起動しました (pid: {pid}, log: {})",
                        state_dir.join("watch-background.log").display()
                    );
                }
                return Ok(ExitCode::SUCCESS);
            }

            let watcher_kind = resolve_distinct_watcher_agent(&config, watcher, allow_same_agent)?;
            let runner = build_runner(watcher_kind, &config)?;
            let (notifier, topic) = build_notifier(&config.notify, &state_dir, dry_run_notify);

            let watch_opts = WatchOptions {
                working_dir: workdir.clone(),
                state_dir: state_dir.clone(),
                timeout_secs: config.watcher.timeout_secs,
                sandbox: Some(config.watcher.sandbox.clone()),
                environment: config.environment.clone(),
                drift: config.drift.clone(),
                notify_topic: topic,
                notify_on_ready_for_manual: config.notify.notify_on_ready_for_manual,
            };

            if let Some(interval_str) = interval {
                let secs = parse_interval_secs(&interval_str)?;
                println!("定期見張りモードを開始します（間隔: {secs} 秒、Ctrl+C で停止）");
                loop {
                    let _ = run_watch_once(
                        &workdir,
                        &scenarios_dir,
                        scenario.as_deref(),
                        &state_dir,
                        if_changed,
                        runner.as_ref(),
                        notifier.as_ref(),
                        &watch_opts,
                    )?;
                    thread::sleep(Duration::from_secs(secs));
                }
            } else {
                let all_ok = run_watch_once(
                    &workdir,
                    &scenarios_dir,
                    scenario.as_deref(),
                    &state_dir,
                    if_changed,
                    runner.as_ref(),
                    notifier.as_ref(),
                    &watch_opts,
                )?;
                Ok(if all_ok {
                    ExitCode::SUCCESS
                } else {
                    ExitCode::FAILURE
                })
            }
        }

        Commands::Check {
            scenario_id,
            check_id,
            pass,
            fail,
            reset,
            reason,
            dir,
            dry_run_notify,
        } => {
            let new_state = if pass {
                CheckState::Passed
            } else if fail {
                CheckState::Failed
            } else if reset {
                CheckState::Pending
            } else {
                bail!("--pass / --fail / --reset のいずれかを指定してください");
            };

            let scenarios_dir = config.resolve_scenarios_dir(&workdir, dir.as_deref());
            let state_dir = config.resolve_state_dir(&workdir);
            let loaded = load_scenarios_from_dir(&scenarios_dir)?;
            let target = loaded
                .into_iter()
                .find(|s| s.scenario.front_matter.id == scenario_id)
                .ok_or_else(|| anyhow::anyhow!("シナリオ `{scenario_id}` が見つかりません"))?;

            let (notifier, topic) = build_notifier(&config.notify, &state_dir, dry_run_notify);
            let (updated, notified) = update_check_manually(
                &target.path,
                &check_id,
                new_state,
                reason,
                notifier.as_ref(),
                &topic,
                config.notify.notify_on_ready_for_manual,
            )?;

            println!(
                "更新完了: {} / {} -> 全体状態: {} (通知: {})",
                updated.front_matter.id,
                check_id,
                updated.front_matter.status,
                if notified { "送信済み" } else { "なし" }
            );
            Ok(ExitCode::SUCCESS)
        }

        Commands::Hook { action } => match action {
            HookAction::Install { bin } => {
                let path = install_post_commit_hook(&workdir, &bin)?;
                println!("git post-commit フックを設置しました: {}", path.display());
                Ok(ExitCode::SUCCESS)
            }
        },

        Commands::Fix {
            dir,
            scenario,
            check_first,
            plan_only,
            auto_approve,
            max_retries,
            developer,
            watcher,
            allow_same_agent,
            dry_run_notify,
        } => {
            let scenarios_dir = config.resolve_scenarios_dir(&workdir, dir.as_deref());
            let state_dir = config.resolve_state_dir(&workdir);
            let dev_kind = developer.unwrap_or(config.agents.developer);
            let watcher_kind = watcher.unwrap_or(config.agents.watcher);

            if dev_kind == watcher_kind && !(allow_same_agent || config.agents.allow_same_agent) {
                bail!(
                    "開発役 ({dev_kind}) と見張り役 ({watcher_kind}) は別エージェントにしてください（--allow-same-agent で上書き可）"
                );
            }

            let dev_runner = build_runner(dev_kind, &config)?;
            let watcher_runner = build_runner(watcher_kind, &config)?;
            let (notifier, topic) = build_notifier(&config.notify, &state_dir, dry_run_notify);

            let options = DevLoopOptions {
                working_dir: workdir.clone(),
                scenarios_dir,
                state_dir,
                target_scenario_id: scenario,
                check_first,
                plan_only,
                auto_approve: auto_approve || config.loop_policy.auto_start_dev,
                max_retries: max_retries.unwrap_or(config.loop_policy.max_retries),
                allow_same_agent: allow_same_agent || config.agents.allow_same_agent,
                watcher_sandbox: Some(config.watcher.sandbox.clone()),
                environment: config.environment.clone(),
                drift: config.drift.clone(),
                timeout_secs: config.watcher.timeout_secs,
                notify_topic: topic,
                notify_on_ready_for_manual: config.notify.notify_on_ready_for_manual,
            };

            let outcome = run_dev_loop(
                dev_runner.as_ref(),
                watcher_runner.as_ref(),
                notifier.as_ref(),
                &options,
                |task| {
                    println!("\n=== 足りない機能のタスクを起票しました ===");
                    println!("ファイル: {}", task.task_file.display());
                    println!(
                        "対象: [{}] {} / Step {} ({})",
                        task.priority,
                        task.scenario_id,
                        task.stopped_step_number,
                        task.failed_check_id
                    );
                    println!("理由: {}", task.failure_reason);
                    prompt_yes_no(&format!(
                        "開発役 ({dev_kind}) に実装を着手させますか？ [y/N]: "
                    ))
                },
            )?;

            match outcome {
                LoopOutcome::NoFailedScenario => {
                    println!("失敗中（failed）のシナリオはありません。");
                    Ok(ExitCode::SUCCESS)
                }
                LoopOutcome::PlannedOnly { task } => {
                    println!("タスクを起票しました: {}", task.task_file.display());
                    Ok(ExitCode::SUCCESS)
                }
                LoopOutcome::DeclinedByUser { task } => {
                    println!(
                        "着手を保留しました（タスクは保存済み: {}）",
                        task.task_file.display()
                    );
                    Ok(ExitCode::SUCCESS)
                }
                LoopOutcome::Resolved {
                    scenario_id,
                    final_status,
                    iterations,
                    last_watch,
                    ..
                } => {
                    println!(
                        "解決しました！ シナリオ `{scenario_id}` -> {final_status} ({iterations} 回で通過 / 通知: {})",
                        if last_watch.notified { "送信" } else { "なし" }
                    );
                    Ok(ExitCode::SUCCESS)
                }
                LoopOutcome::MaxRetriesReached {
                    scenario_id,
                    iterations,
                    last_watch,
                    ..
                } => {
                    eprintln!(
                        "上限 ({iterations} 回) に達しましたが `{scenario_id}` はまだ未通過です（停止: Step {:?} / {:?}）",
                        last_watch.stopped_step, last_watch.stopped_check
                    );
                    Ok(ExitCode::FAILURE)
                }
            }
        }

        Commands::Notify { action } => match action {
            NotifyAction::Test { topic, dry_run } => {
                let state_dir = config.resolve_state_dir(&workdir);
                let mut notify_cfg = config.notify.clone();
                if let Some(t) = topic {
                    notify_cfg.topic = Some(t);
                }
                if !dry_run && resolve_topic(&notify_cfg).is_empty() {
                    bail!(
                        "実送信には --topic <name> または環境変数 {} の設定が必要です（テスト記録のみなら --dry-run を使用）",
                        notify_cfg.topic_env
                    );
                }
                let (notifier, resolved_topic) = build_notifier(&notify_cfg, &state_dir, dry_run);
                let payload = NotificationPayload {
                    scenario_id: "notify-test".to_string(),
                    title: "[TEST] Daihon 通知テスト".to_string(),
                    priority: Priority::P1,
                    status: ScenarioStatus::Passed,
                    checked_at: Local::now().to_rfc3339(),
                    message: "シナリオ駆動の開発ハーネスからのテスト通知です（本番では全ステップ通過時のみ届きます）".to_string(),
                    topic: resolved_topic.clone(),
                };
                notifier.send(&payload)?;
                if dry_run || notify_cfg.dry_run {
                    println!(
                        "ドライラン通知を記録しました: {}",
                        state_dir.join("notifications.jsonl").display()
                    );
                } else {
                    println!("ntfy へテスト通知を送信しました (topic: {resolved_topic})");
                }
                Ok(ExitCode::SUCCESS)
            }
        },

        Commands::InitIntegration {
            target,
            project,
            no_rules,
            no_hooks,
        } => {
            let dest = project.unwrap_or(workdir);
            let files = install_integration_with_options(
                target,
                &dest,
                InstallOptions {
                    include_rules: !no_rules,
                    include_hooks: !no_hooks,
                },
            )?;
            println!("{target} 連携ファイルを設置しました:");
            for f in files {
                println!("  - {}", f.display());
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn cmd_validate(path: &Path, as_json: bool) -> ExitCode {
    let report = validate_path(path);

    if as_json {
        match serde_json::to_string_pretty(&report) {
            Ok(out) => println!("{out}"),
            Err(err) => {
                eprintln!("JSON シリアライズに失敗しました: {err}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        for file in &report.files {
            if file.valid {
                let id = file.scenario_id.as_deref().unwrap_or("-");
                println!("OK  {} (id: {})", file.path.display(), id);
            } else {
                println!("NG  {}", file.path.display());
                for issue in &file.issues {
                    println!("    - {issue}");
                }
            }
        }

        for issue in &report.cross_file_issues {
            println!("NG  {issue}");
        }

        if report.is_ok() {
            println!("\n形式チェック完了: {} 件すべて OK", report.files.len());
        } else {
            eprintln!(
                "\n形式チェック失敗: {} 件のエラーが見つかりました",
                report.total_issues()
            );
        }
    }

    if report.is_ok() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn cmd_status(
    scenarios_dir: &Path,
    state_dir: &Path,
    drift_cfg: &DriftConfig,
    as_json: bool,
) -> Result<()> {
    let loaded = load_scenarios_from_dir(scenarios_dir)?;
    let mut rows = Vec::with_capacity(loaded.len());
    for item in loaded {
        let sc = item.scenario;
        let passed_steps = sc
            .steps
            .iter()
            .filter(|s| matches!(s.status, StepStatus::Passed | StepStatus::ReadyForManual))
            .count();
        let stopped = sc.steps.iter().find(|s| s.status == StepStatus::Failed);
        let drift = analyze_scenario_drift(state_dir, &sc.front_matter.id, drift_cfg)?;
        rows.push(StatusSummaryRow {
            id: sc.front_matter.id,
            priority: sc.front_matter.priority,
            status: sc.front_matter.status,
            title: sc.front_matter.title,
            passed_steps,
            total_steps: sc.steps.len(),
            stopped_step: stopped.map(|s| s.number),
            failed_check: stopped.and_then(|s| s.failed_check.clone()),
            drift,
            last_checked_at: sc.front_matter.last_checked_at,
            last_checked_by: sc.front_matter.last_checked_by,
            path: item.path,
        });
    }

    if as_json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }

    if rows.is_empty() {
        println!(
            "シナリオはまだ登録されていません（{}）",
            scenarios_dir.display()
        );
        return Ok(());
    }

    for row in &rows {
        let badge = match row.status {
            ScenarioStatus::Passed => "PASS",
            ScenarioStatus::Failed => "FAIL",
            ScenarioStatus::ReadyForManual => "WAIT",
            ScenarioStatus::InProgress => "RUN ",
            ScenarioStatus::Pending => "TODO",
        };
        let stop_note = match (row.stopped_step, &row.failed_check) {
            (Some(step), Some(cid)) => format!("  [停止: Step {step} / {cid}]"),
            (Some(step), None) => format!("  [停止: Step {step}]"),
            _ => String::new(),
        };
        let drift_note = match &row.drift {
            Some(d) if d.is_drifting => format!("  [DRIFT x{}]", d.consecutive_same_stop),
            _ => String::new(),
        };
        println!(
            "[{badge}] {} ({}/{}) {} — {}{stop_note}{drift_note}",
            row.priority, row.passed_steps, row.total_steps, row.id, row.title
        );
    }

    let next_task = state_dir.join("NEXT_TASK.md");
    if next_task.exists() && rows.iter().any(|r| r.status == ScenarioStatus::Failed) {
        println!("\n次の優先タスク要約: {}", next_task.display());
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn run_watch_once(
    workdir: &Path,
    scenarios_dir: &Path,
    scenario_filter: Option<&str>,
    state_dir: &Path,
    if_changed: bool,
    runner: &dyn daihon::runner::AgentRunner,
    notifier: &dyn daihon::notify::Notifier,
    options: &WatchOptions,
) -> Result<bool> {
    if if_changed && is_unchanged_since_last_watch(workdir, scenarios_dir, state_dir) {
        println!("前回の見張り以降にプロダクトコードの変化がないためスキップしました。");
        return Ok(true);
    }

    let Some(_lock) = RunLock::try_acquire(state_dir)? else {
        println!("別の見張りプロセスが実行中のため今回の実行をスキップしました。");
        return Ok(true);
    };

    let loaded = load_scenarios_from_dir(scenarios_dir)?;
    let targets: Vec<_> = loaded
        .into_iter()
        .filter(|s| scenario_filter.is_none_or(|id| s.scenario.front_matter.id == id))
        .collect();

    if targets.is_empty() {
        bail!("検証対象のシナリオが見つかりません");
    }

    let mut all_passed = true;
    for target in targets {
        println!(
            "見張り実行中 ({}) : [{}] {}",
            runner.name(),
            target.scenario.front_matter.priority,
            target.scenario.front_matter.id
        );
        let report = watch_scenario(&target.path, runner, notifier, options)?;
        if let Some(step) = report.stopped_step {
            all_passed = false;
            println!(
                "  -> FAIL: Step {step} ({}) で停止: {}",
                report.stopped_check.as_deref().unwrap_or("-"),
                report.stopped_reason.as_deref().unwrap_or("-")
            );
            if let Some(ref drift) = report.drift
                && drift.is_drifting
            {
                println!("  -> ⚠ ドリフト検知: {}", drift.reason);
            }
            println!("     更新シナリオ: {}", report.scenario_path.display());
            println!("     詳細ログ    : {}", report.report_path.display());
            println!(
                "     優先タスク  : {}",
                state_dir.join("NEXT_TASK.md").display()
            );
            println!(
                "     次の操作    : `daihon status` / `daihon tui` で確認、`daihon fix --plan-only` でタスク起票"
            );
        } else {
            println!(
                "  -> {} (通知: {})",
                report.new_status,
                if report.notified { "送信" } else { "なし" }
            );
            println!("     更新シナリオ: {}", report.scenario_path.display());
            println!("     詳細ログ    : {}", report.report_path.display());
        }
    }

    let fp = compute_worktree_fingerprint(workdir, Some(scenarios_dir), Some(state_dir));
    let _ = record_last_watch_fingerprint(state_dir, &fp.fingerprint);

    Ok(all_passed)
}

fn read_register_input(text_arg: Option<String>) -> Result<String> {
    if let Some(t) = text_arg {
        return Ok(t);
    }

    if !std::io::IsTerminal::is_terminal(&io::stdin()) {
        let mut buf = String::new();
        io::stdin().read_to_string(&mut buf)?;
        return Ok(buf);
    }

    print!("登録したいシナリオを自然言語で入力してください: ");
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    Ok(line)
}

enum ConfirmAction {
    Save(Option<Priority>),
    Cancel,
}

fn prompt_registration_confirmation(default_priority: Priority) -> Result<ConfirmAction> {
    print!(
        "\nこの内容で保存しますか？ [y=保存 / P0..P3=優先度を変更して保存 / n=キャンセル] (既定: y, {default_priority}): "
    );
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let trimmed = input.trim();
    if trimmed.is_empty()
        || trimmed.eq_ignore_ascii_case("y")
        || trimmed.eq_ignore_ascii_case("yes")
    {
        return Ok(ConfirmAction::Save(None));
    }
    if trimmed.eq_ignore_ascii_case("n") || trimmed.eq_ignore_ascii_case("no") {
        return Ok(ConfirmAction::Cancel);
    }
    if let Ok(p) = trimmed.to_ascii_uppercase().parse::<Priority>() {
        return Ok(ConfirmAction::Save(Some(p)));
    }
    Ok(ConfirmAction::Cancel)
}

fn prompt_yes_no(prompt: &str) -> Result<bool> {
    print!("{prompt}");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let trimmed = input.trim();
    Ok(trimmed.eq_ignore_ascii_case("y") || trimmed.eq_ignore_ascii_case("yes"))
}
