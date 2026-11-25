use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::config::DriftConfig;
use crate::model::scenario::ScenarioStatus;
use crate::model::store::{LoadedScenario, TreeEntry, load_scenarios_tree, order_as_tree};
use crate::watcher::drift::{DriftSummary, analyze_scenario_drift};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusPane {
    List,
    Detail,
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
    pub entries: Vec<TreeEntry>,
    pub drifts: HashMap<String, DriftSummary>,
    pub selected: usize,
    pub detail_scroll: u16,
    pub focus: FocusPane,
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
        let entries = load_scenarios_tree(dir)?;
        let drifts = load_drift_map(&entries, state_dir, &drift_cfg);
        Ok(Self {
            dir: dir.to_path_buf(),
            state_dir: state_dir.to_path_buf(),
            drift_cfg,
            entries,
            drifts,
            selected: 0,
            detail_scroll: 0,
            focus: FocusPane::List,
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
            entries: order_as_tree(scenarios),
            drifts: HashMap::new(),
            selected: 0,
            detail_scroll: 0,
            focus: FocusPane::List,
            status_message: None,
        }
    }

    pub fn reload(&mut self) {
        match load_scenarios_tree(&self.dir) {
            Ok(entries) => {
                let prev_id = self
                    .selected_scenario()
                    .map(|s| s.scenario.front_matter.id.clone());
                self.drifts = load_drift_map(&entries, &self.state_dir, &self.drift_cfg);
                self.entries = entries;
                if let Some(id) = prev_id
                    && let Some(pos) = self
                        .entries
                        .iter()
                        .position(|e| e.loaded.scenario.front_matter.id == id)
                {
                    self.selected = pos;
                } else if self.selected >= self.entries.len() {
                    self.selected = self.entries.len().saturating_sub(1);
                }
                self.detail_scroll = 0;
                self.status_message = Some(format!("再読込完了 ({} 件)", self.entries.len()));
            }
            Err(err) => {
                self.status_message = Some(format!("読込エラー: {err}"));
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
                    self.detail_scroll = 0;
                }
            }
            FocusPane::Detail => {
                self.detail_scroll = self.detail_scroll.saturating_add(1);
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
                    self.detail_scroll = 0;
                }
            }
            FocusPane::Detail => {
                self.detail_scroll = self.detail_scroll.saturating_sub(1);
            }
        }
    }

    pub fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            FocusPane::List => FocusPane::Detail,
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
