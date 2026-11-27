use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::config::{DriftConfig, NotifyConfig};
use crate::model::scenario::{CheckState, Priority, ScenarioStatus, VerifyMode};
use crate::model::store::{
    LoadedScenario, TreeEntry, load_scenarios_tree, order_as_tree, save_updated_scenario,
};
use crate::notify::build_notifier;
use crate::register::RegistrationProposal;
use crate::watcher::drift::{
    DriftSummary, WatchAbortRecord, analyze_scenario_drift, load_watch_abort,
};
use crate::watcher::update_check_manually;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusPane {
    List,
    Detail,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DraftTarget {
    NewScenario,
    Togaki {
        parent_id: String,
        parent_title: String,
    },
}

#[derive(Debug, Clone)]
pub enum ModalState {
    None,
    Input {
        target: DraftTarget,
        input: String,
    },
    Generating {
        target: DraftTarget,
        input: String,
        tick: usize,
    },
    Preview {
        target: DraftTarget,
        proposal: Box<RegistrationProposal>,
        priority: Priority,
        scroll: u16,
    },
}

#[derive(Debug, Clone, Default)]
pub struct ChildrenSummary {
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub pending: usize,
}

#[derive(Debug, Clone)]
pub struct App {
    pub dir: PathBuf,
    pub state_dir: PathBuf,
    pub drift_cfg: DriftConfig,
    pub notify_cfg: NotifyConfig,
    pub entries: Vec<TreeEntry>,
    pub drifts: HashMap<String, DriftSummary>,
    pub aborts: HashMap<String, WatchAbortRecord>,
    pub selected: usize,
    pub selected_check: usize,
    pub detail_scroll: u16,
    pub focus: FocusPane,
    pub modal: ModalState,
    pub status_message: Option<String>,
}

impl App {
    pub fn load(dir: &Path) -> Result<Self> {
        let state_dir = dir
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(".daihon");
        Self::load_with_state(dir, &state_dir, DriftConfig::default())
    }

    pub fn load_with_state(dir: &Path, state_dir: &Path, drift_cfg: DriftConfig) -> Result<Self> {
        Self::load_with_config(dir, state_dir, drift_cfg, NotifyConfig::default())
    }

    pub fn load_with_config(
        dir: &Path,
        state_dir: &Path,
        drift_cfg: DriftConfig,
        notify_cfg: NotifyConfig,
    ) -> Result<Self> {
        let entries = load_scenarios_tree(dir)?;
        let drifts = load_drift_map(&entries, state_dir, &drift_cfg);
        let aborts = load_abort_map(&entries, state_dir);
        Ok(Self {
            dir: dir.to_path_buf(),
            state_dir: state_dir.to_path_buf(),
            drift_cfg,
            notify_cfg,
            entries,
            drifts,
            aborts,
            selected: 0,
            selected_check: 0,
            detail_scroll: 0,
            focus: FocusPane::List,
            modal: ModalState::None,
            status_message: None,
        })
    }

    pub fn from_scenarios(dir: PathBuf, scenarios: Vec<LoadedScenario>) -> Self {
        Self {
            state_dir: dir
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join(".daihon"),
            dir,
            drift_cfg: DriftConfig::default(),
            notify_cfg: NotifyConfig::default(),
            entries: order_as_tree(scenarios),
            drifts: HashMap::new(),
            aborts: HashMap::new(),
            selected: 0,
            selected_check: 0,
            detail_scroll: 0,
            focus: FocusPane::List,
            modal: ModalState::None,
            status_message: None,
        }
    }

    pub fn reload(&mut self) {
        self.reload_and_select(None);
    }

    pub fn reload_and_select(&mut self, select_id: Option<&str>) {
        match load_scenarios_tree(&self.dir) {
            Ok(entries) => {
                let target_id = select_id.map(ToOwned::to_owned).or_else(|| {
                    self.selected_scenario()
                        .map(|s| s.scenario.front_matter.id.clone())
                });
                self.drifts = load_drift_map(&entries, &self.state_dir, &self.drift_cfg);
                self.aborts = load_abort_map(&entries, &self.state_dir);
                self.entries = entries;
                if let Some(id) = target_id
                    && let Some(pos) = self
                        .entries
                        .iter()
                        .position(|e| e.loaded.scenario.front_matter.id == id)
                {
                    self.selected = pos;
                } else if self.selected >= self.entries.len() {
                    self.selected = self.entries.len().saturating_sub(1);
                }
                let total_checks = self.total_checks_in_selected();
                if total_checks == 0 {
                    self.selected_check = 0;
                } else if self.selected_check >= total_checks {
                    self.selected_check = total_checks - 1;
                }
                self.detail_scroll = 0;
                if select_id.is_none() {
                    let (scenarios, togakis) = self.counts();
                    self.status_message = if togakis > 0 {
                        Some(format!(
                            "再読込完了 (シナリオ {scenarios} / ト書き {togakis})"
                        ))
                    } else {
                        Some(format!("再読込完了 (シナリオ {scenarios})"))
                    };
                }
            }
            Err(err) => {
                self.status_message = Some(format!("読込エラー: {err}"));
            }
        }
    }

    /// Returns `(root_scenario_count, togaki_count)`.
    pub fn counts(&self) -> (usize, usize) {
        let togaki = self
            .entries
            .iter()
            .filter(|e| e.loaded.scenario.is_togaki())
            .count();
        (self.entries.len().saturating_sub(togaki), togaki)
    }

    /// Surgically updates the selected scenario/Togaki's priority on disk and re-sorts the tree.
    pub fn set_selected_priority(&mut self, new_priority: Priority) {
        let Some(selected) = self.selected_scenario().cloned() else {
            return;
        };
        let id = selected.scenario.front_matter.id.clone();
        if selected.scenario.front_matter.priority == new_priority {
            self.status_message = Some(format!("{id} はすでに {new_priority} です"));
            return;
        }
        let mut updated = selected.scenario;
        updated.front_matter.priority = new_priority;
        match save_updated_scenario(&selected.path, &updated) {
            Ok(()) => {
                self.reload_and_select(Some(&id));
                self.status_message = Some(format!("優先度を変更しました: {id} -> {new_priority}"));
            }
            Err(err) => {
                self.status_message = Some(format!("優先度変更エラー: {err}"));
            }
        }
    }

    pub fn selected_entry(&self) -> Option<&TreeEntry> {
        self.entries.get(self.selected)
    }

    pub fn selected_scenario(&self) -> Option<&LoadedScenario> {
        self.entries.get(self.selected).map(|e| &e.loaded)
    }

    pub fn drift_for(&self, scenario_id: &str) -> Option<&DriftSummary> {
        self.drifts.get(scenario_id)
    }

    pub fn abort_for(&self, scenario_id: &str) -> Option<&WatchAbortRecord> {
        self.aborts.get(scenario_id)
    }

    pub fn total_checks_in_selected(&self) -> usize {
        self.selected_scenario()
            .map(|s| s.scenario.steps.iter().map(|st| st.checks.len()).sum())
            .unwrap_or(0)
    }

    pub fn selected_check_info(&self) -> Option<(String, VerifyMode, CheckState)> {
        let sc = &self.selected_scenario()?.scenario;
        sc.steps
            .iter()
            .flat_map(|st| st.checks.iter())
            .nth(self.selected_check)
            .map(|c| (c.id.clone(), c.mode, c.state))
    }

    /// Switches focus directly to Detail pane and positions the check cursor on the first
    /// `[manual]` check item (or check 0 if none exist).
    pub fn focus_manual_checks(&mut self) {
        if self.selected_scenario().is_none() {
            return;
        }
        self.focus = FocusPane::Detail;
        if let Some(sc) = self.selected_scenario().map(|s| &s.scenario)
            && let Some(pos) = sc
                .steps
                .iter()
                .flat_map(|st| st.checks.iter())
                .position(|c| c.mode == VerifyMode::Manual && c.state != CheckState::Passed)
                .or_else(|| {
                    sc.steps
                        .iter()
                        .flat_map(|st| st.checks.iter())
                        .position(|c| c.mode == VerifyMode::Manual)
                })
        {
            self.selected_check = pos;
        }
    }

    /// Toggles the currently highlighted check in Detail pane (`Passed` <-> `Pending`),
    /// persisting surgically to Markdown and firing full-pass `ntfy` if all checks pass.
    pub fn toggle_selected_check_passed(&mut self) {
        let Some((_, _, current_state)) = self.selected_check_info() else {
            return;
        };
        let next_state = if current_state == CheckState::Passed {
            CheckState::Pending
        } else {
            CheckState::Passed
        };
        self.set_selected_check_state(next_state);
    }

    pub fn set_selected_check_state(&mut self, new_state: CheckState) {
        let Some(selected) = self.selected_scenario().cloned() else {
            return;
        };
        let Some((check_id, _, _)) = self.selected_check_info() else {
            return;
        };
        let scenario_id = selected.scenario.front_matter.id.clone();
        let kept_check_idx = self.selected_check;
        let (notifier, topic) =
            build_notifier(&self.notify_cfg, &self.state_dir, self.notify_cfg.dry_run);
        match update_check_manually(
            &selected.path,
            &check_id,
            new_state,
            None,
            notifier.as_ref(),
            &topic,
            self.notify_cfg.notify_on_ready_for_manual,
        ) {
            Ok((updated, notified)) => {
                self.reload_and_select(Some(&scenario_id));
                let max_idx = self.total_checks_in_selected().saturating_sub(1);
                self.selected_check = kept_check_idx.min(max_idx);
                let notify_note = if notified { " / 通知送信" } else { "" };
                self.status_message = Some(format!(
                    "チェック更新: {scenario_id}/{check_id} -> [{}] (全体: {}{notify_note})",
                    new_state.marker(),
                    updated.front_matter.status
                ));
            }
            Err(err) => {
                self.status_message = Some(format!("チェック更新エラー: {err}"));
            }
        }
    }

    pub fn scroll_detail_down(&mut self) {
        self.detail_scroll = self.detail_scroll.saturating_add(3);
    }

    pub fn scroll_detail_up(&mut self) {
        self.detail_scroll = self.detail_scroll.saturating_sub(3);
    }

    /// Resolves the parent scenario to attach a new Togaki to based on the current selection.
    /// If a child Togaki is selected, resolves to its parent scenario so users can rapidly add
    /// multiple sibling Togaki without accidentally nesting deeper.
    pub fn resolve_togaki_parent(&self) -> Option<(String, String)> {
        let sc = &self.selected_entry()?.loaded.scenario;
        if let Some(ref pid) = sc.front_matter.parent
            && let Some(parent_entry) = self
                .entries
                .iter()
                .find(|e| e.loaded.scenario.front_matter.id == *pid)
        {
            return Some((
                parent_entry.loaded.scenario.front_matter.id.clone(),
                parent_entry.loaded.scenario.front_matter.title.clone(),
            ));
        }
        Some((
            sc.front_matter.id.clone(),
            sc.front_matter.title.clone(),
        ))
    }

    pub fn open_togaki_modal(&mut self) {
        if let Some((parent_id, parent_title)) = self.resolve_togaki_parent() {
            self.modal = ModalState::Input {
                target: DraftTarget::Togaki {
                    parent_id,
                    parent_title,
                },
                input: String::new(),
            };
        } else {
            self.status_message =
                Some("ト書きを紐づける親シナリオがありません（先に `n` でシナリオを登録してください）".to_string());
        }
    }

    pub fn open_new_scenario_modal(&mut self) {
        self.modal = ModalState::Input {
            target: DraftTarget::NewScenario,
            input: String::new(),
        };
    }

    pub fn close_modal(&mut self, msg: Option<String>) {
        self.modal = ModalState::None;
        if msg.is_some() {
            self.status_message = msg;
        }
    }

    pub fn children_summary_for(&self, parent_id: &str) -> ChildrenSummary {
        let mut summary = ChildrenSummary::default();
        for entry in &self.entries {
            if entry.loaded.scenario.front_matter.parent.as_deref() == Some(parent_id) {
                summary.total += 1;
                match entry.loaded.scenario.front_matter.status {
                    ScenarioStatus::Passed | ScenarioStatus::ReadyForManual => summary.passed += 1,
                    ScenarioStatus::Failed => summary.failed += 1,
                    ScenarioStatus::Pending | ScenarioStatus::InProgress => summary.pending += 1,
                }
            }
        }
        summary
    }

    pub fn next(&mut self) {
        match self.focus {
            FocusPane::List => {
                if !self.entries.is_empty() {
                    self.selected = (self.selected + 1) % self.entries.len();
                    self.selected_check = 0;
                    self.detail_scroll = 0;
                }
            }
            FocusPane::Detail => {
                let total = self.total_checks_in_selected();
                if total > 0 {
                    self.selected_check = (self.selected_check + 1) % total;
                } else {
                    self.detail_scroll = self.detail_scroll.saturating_add(1);
                }
            }
        }
    }

    pub fn previous(&mut self) {
        match self.focus {
            FocusPane::List => {
                if !self.entries.is_empty() {
                    if self.selected == 0 {
                        self.selected = self.entries.len() - 1;
                    } else {
                        self.selected -= 1;
                    }
                    self.selected_check = 0;
                    self.detail_scroll = 0;
                }
            }
            FocusPane::Detail => {
                let total = self.total_checks_in_selected();
                if total > 0 {
                    if self.selected_check == 0 {
                        self.selected_check = total - 1;
                    } else {
                        self.selected_check -= 1;
                    }
                } else {
                    self.detail_scroll = self.detail_scroll.saturating_sub(1);
                }
            }
        }
    }

    pub fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            FocusPane::List => {
                // Default to first unpassed [manual] check if any when entering Detail pane.
                if let Some(sc) = self.selected_scenario().map(|s| &s.scenario)
                    && let Some(pos) = sc
                        .steps
                        .iter()
                        .flat_map(|st| st.checks.iter())
                        .position(|c| c.mode == VerifyMode::Manual && c.state != CheckState::Passed)
                {
                    self.selected_check = pos;
                }
                FocusPane::Detail
            }
            FocusPane::Detail => FocusPane::List,
        };
    }
}

fn load_drift_map(
    entries: &[TreeEntry],
    state_dir: &Path,
    drift_cfg: &DriftConfig,
) -> HashMap<String, DriftSummary> {
    let mut map = HashMap::new();
    for entry in entries {
        let id = &entry.loaded.scenario.front_matter.id;
        if let Ok(Some(summary)) = analyze_scenario_drift(state_dir, id, drift_cfg) {
            map.insert(id.clone(), summary);
        }
    }
    map
}

fn load_abort_map(entries: &[TreeEntry], state_dir: &Path) -> HashMap<String, WatchAbortRecord> {
    let mut map = HashMap::new();
    for entry in entries {
        let id = &entry.loaded.scenario.front_matter.id;
        if let Some(rec) = load_watch_abort(state_dir, id) {
            map.insert(id.clone(), rec);
        }
    }
    map
}
