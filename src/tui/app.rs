use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::config::DriftConfig;
use crate::model::store::{LoadedScenario, load_scenarios_from_dir};
use crate::watcher::drift::{DriftSummary, analyze_scenario_drift};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusPane {
    List,
    Detail,
}

#[derive(Debug, Clone)]
pub struct App {
    pub dir: PathBuf,
    pub state_dir: PathBuf,
    pub drift_cfg: DriftConfig,
    pub scenarios: Vec<LoadedScenario>,
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
        let scenarios = load_scenarios_from_dir(dir)?;
        let drifts = load_drift_map(&scenarios, state_dir, &drift_cfg);
        Ok(Self {
            dir: dir.to_path_buf(),
            state_dir: state_dir.to_path_buf(),
            drift_cfg,
            scenarios,
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
            scenarios,
            drifts: HashMap::new(),
            selected: 0,
            detail_scroll: 0,
            focus: FocusPane::List,
            status_message: None,
        }
    }

    pub fn reload(&mut self) {
        match load_scenarios_from_dir(&self.dir) {
            Ok(scenarios) => {
                let prev_id = self
                    .selected_scenario()
                    .map(|s| s.scenario.front_matter.id.clone());
                self.drifts = load_drift_map(&scenarios, &self.state_dir, &self.drift_cfg);
                self.scenarios = scenarios;
                if let Some(id) = prev_id
                    && let Some(pos) = self
                        .scenarios
                        .iter()
                        .position(|s| s.scenario.front_matter.id == id)
                {
                    self.selected = pos;
                } else if self.selected >= self.scenarios.len() {
                    self.selected = self.scenarios.len().saturating_sub(1);
                }
                self.detail_scroll = 0;
                self.status_message = Some(format!("再読込完了 ({} 件)", self.scenarios.len()));
            }
            Err(err) => {
                self.status_message = Some(format!("読込エラー: {err}"));
            }
        }
    }

    pub fn selected_scenario(&self) -> Option<&LoadedScenario> {
        self.scenarios.get(self.selected)
    }

    pub fn drift_for(&self, scenario_id: &str) -> Option<&DriftSummary> {
        self.drifts.get(scenario_id)
    }

    pub fn next(&mut self) {
        match self.focus {
            FocusPane::List => {
                if !self.scenarios.is_empty() {
                    self.selected = (self.selected + 1) % self.scenarios.len();
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
                if !self.scenarios.is_empty() {
                    if self.selected == 0 {
                        self.selected = self.scenarios.len() - 1;
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
    scenarios: &[LoadedScenario],
    state_dir: &Path,
    drift_cfg: &DriftConfig,
) -> HashMap<String, DriftSummary> {
    let mut map = HashMap::new();
    for item in scenarios {
        let id = &item.scenario.front_matter.id;
        if let Ok(Some(summary)) = analyze_scenario_drift(state_dir, id, drift_cfg) {
            map.insert(id.clone(), summary);
        }
    }
    map
}
