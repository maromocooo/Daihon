pub mod app;
pub mod render;

use std::io;
use std::path::Path;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyModifiers,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::config::HarnessConfig;
use crate::model::Priority;
use crate::register::{RegistrationProposal, propose_registration, propose_togaki, save_registration};
use crate::runner::build_runner;
use crate::tui::app::{App, DraftTarget, ModalState};

struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), DisableBracketedPaste, LeaveAlternateScreen);
        let _ = disable_raw_mode();
    }
}

/// Launches the interactive minimal scenario TUI for `scenarios_dir`,
/// including direct `t` (add Togaki) and `n` (new scenario) registration modals.
pub fn run_tui(workdir: &Path, scenarios_dir: &Path, config: &HarnessConfig) -> Result<()> {
    let state_dir = config.resolve_state_dir(workdir);
    let mut app = App::load_with_state(scenarios_dir, &state_dir, config.drift.clone())?;

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableBracketedPaste)?;
    let _guard = TerminalGuard;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut pending_rx: Option<Receiver<Result<RegistrationProposal>>> = None;
    let mut last_modal_discriminant = std::mem::discriminant(&app.modal);

    loop {
        // Poll background registration worker if active.
        if let Some(ref rx) = pending_rx {
            match rx.try_recv() {
                Ok(Ok(proposal)) => {
                    pending_rx = None;
                    if let ModalState::Generating { target, .. } = &app.modal {
                        let suggested = proposal.priority_proposal.suggested;
                        app.modal = ModalState::Preview {
                            target: target.clone(),
                            proposal: Box::new(proposal),
                            priority: suggested,
                            scroll: 0,
                        };
                    }
                }
                Ok(Err(err)) => {
                    pending_rx = None;
                    app.close_modal(Some(format!("生成エラー: {err}")));
                }
                Err(TryRecvError::Empty) => {
                    if let ModalState::Generating { tick, .. } = &mut app.modal {
                        *tick = tick.wrapping_add(1);
                    }
                }
                Err(TryRecvError::Disconnected) => {
                    pending_rx = None;
                    app.close_modal(Some("生成ワーカーが中断されました".to_string()));
                }
            }
        }

        let current_disc = std::mem::discriminant(&app.modal);
        if current_disc != last_modal_discriminant {
            let _ = terminal.clear();
            last_modal_discriminant = current_disc;
        }

        terminal.draw(|frame| render::render(frame, &app))?;

        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Paste(pasted) => {
                    if let ModalState::Input { input, .. } = &mut app.modal {
                        let single_line = pasted.replace(['\r', '\n'], " ");
                        input.push_str(&single_line);
                    }
                }
                Event::Key(key) => {
                    if key.code == KeyCode::Char('c')
                        && key.modifiers.contains(KeyModifiers::CONTROL)
                    {
                        break;
                    }

                    match &mut app.modal {
                        ModalState::None => match key.code {
                            KeyCode::Char('q') | KeyCode::Esc => break,
                            KeyCode::Down | KeyCode::Char('j') => app.next(),
                            KeyCode::Up | KeyCode::Char('k') => app.previous(),
                            KeyCode::Tab => app.toggle_focus(),
                            KeyCode::Char('r') => app.reload(),
                            KeyCode::Char('t') => app.open_togaki_modal(),
                            KeyCode::Char('n') => app.open_new_scenario_modal(),
                            _ => {}
                        },
                        ModalState::Input { target, input } => match key.code {
                            KeyCode::Esc => {
                                app.close_modal(Some("入力をキャンセルしました".to_string()));
                            }
                            KeyCode::Backspace => {
                                input.pop();
                            }
                            KeyCode::Enter => {
                                let text = input.trim().to_string();
                                if text.is_empty() {
                                    app.close_modal(Some("入力が空のためキャンセルしました".to_string()));
                                } else {
                                    let target_clone = target.clone();
                                    let rx = spawn_proposal_worker(
                                        target_clone.clone(),
                                        text.clone(),
                                        workdir.to_path_buf(),
                                        app.dir.clone(),
                                        config.clone(),
                                    );
                                    pending_rx = Some(rx);
                                    app.modal = ModalState::Generating {
                                        target: target_clone,
                                        input: text,
                                        tick: 0,
                                    };
                                }
                            }
                            KeyCode::Char(c)
                                if !key.modifiers.contains(KeyModifiers::CONTROL)
                                    && !key.modifiers.contains(KeyModifiers::ALT) =>
                            {
                                input.push(c);
                            }
                            _ => {}
                        },
                        ModalState::Generating { .. } => {
                            if key.code == KeyCode::Esc {
                                pending_rx = None;
                                app.close_modal(Some("生成をキャンセルしました".to_string()));
                            }
                        }
                        ModalState::Preview {
                            proposal,
                            priority,
                            scroll,
                            ..
                        } => match key.code {
                            KeyCode::Esc | KeyCode::Char('n') => {
                                app.close_modal(Some("保存をキャンセルしました".to_string()));
                            }
                            KeyCode::Down | KeyCode::Char('j') => {
                                *scroll = scroll.saturating_add(1);
                            }
                            KeyCode::Up | KeyCode::Char('k') => {
                                *scroll = scroll.saturating_sub(1);
                            }
                            KeyCode::Char('0') => *priority = Priority::P0,
                            KeyCode::Char('1') => *priority = Priority::P1,
                            KeyCode::Char('2') => *priority = Priority::P2,
                            KeyCode::Char('3') => *priority = Priority::P3,
                            KeyCode::Enter | KeyCode::Char('y') => {
                                let chosen = *priority;
                                let prop_clone = proposal.as_ref().clone();
                                let new_id = prop_clone.scenario.front_matter.id.clone();
                                match save_registration(&prop_clone, &app.dir, Some(chosen)) {
                                    Ok(saved_path) => {
                                        app.modal = ModalState::None;
                                        app.reload_and_select(Some(&new_id));
                                        let fname = saved_path
                                            .file_name()
                                            .and_then(|n| n.to_str())
                                            .unwrap_or("saved");
                                        app.status_message = Some(format!("保存しました: {fname}"));
                                    }
                                    Err(err) => {
                                        app.close_modal(Some(format!("保存エラー: {err}")));
                                    }
                                }
                            }
                            _ => {}
                        },
                    }
                }
                _ => {}
            }
        }
    }

    Ok(())
}

fn spawn_proposal_worker(
    target: DraftTarget,
    natural_input: String,
    workdir: std::path::PathBuf,
    scenarios_dir: std::path::PathBuf,
    config: HarnessConfig,
) -> Receiver<Result<RegistrationProposal>> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let res = (|| -> Result<RegistrationProposal> {
            let runner = build_runner(config.agents.register, &config)?;
            match target {
                DraftTarget::NewScenario => propose_registration(
                    &natural_input,
                    &scenarios_dir,
                    runner.as_ref(),
                    &workdir,
                    config.watcher.timeout_secs,
                ),
                DraftTarget::Togaki { parent_id, .. } => propose_togaki(
                    &parent_id,
                    &natural_input,
                    None,
                    &scenarios_dir,
                    runner.as_ref(),
                    &workdir,
                    config.watcher.timeout_secs,
                ),
            }
        })();
        let _ = tx.send(res);
    });
    rx
}
