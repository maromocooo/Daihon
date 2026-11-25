use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Priority {
    P0,
    P1,
    P2,
    P3,
}

impl fmt::Display for Priority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Priority::P0 => f.write_str("P0"),
            Priority::P1 => f.write_str("P1"),
            Priority::P2 => f.write_str("P2"),
            Priority::P3 => f.write_str("P3"),
        }
    }
}

impl FromStr for Priority {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "P0" => Ok(Priority::P0),
            "P1" => Ok(Priority::P1),
            "P2" => Ok(Priority::P2),
            "P3" => Ok(Priority::P3),
            _ => Err(format!(
                "invalid priority '{s}' (expected one of: P0, P1, P2, P3)"
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScenarioStatus {
    Pending,
    InProgress,
    Passed,
    Failed,
    ReadyForManual,
}

impl fmt::Display for ScenarioStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScenarioStatus::Pending => f.write_str("pending"),
            ScenarioStatus::InProgress => f.write_str("in_progress"),
            ScenarioStatus::Passed => f.write_str("passed"),
            ScenarioStatus::Failed => f.write_str("failed"),
            ScenarioStatus::ReadyForManual => f.write_str("ready_for_manual"),
        }
    }
}

impl FromStr for ScenarioStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pending" => Ok(ScenarioStatus::Pending),
            "in_progress" => Ok(ScenarioStatus::InProgress),
            "passed" => Ok(ScenarioStatus::Passed),
            "failed" => Ok(ScenarioStatus::Failed),
            "ready_for_manual" => Ok(ScenarioStatus::ReadyForManual),
            _ => Err(format!(
                "invalid daihon status '{s}' (expected one of: pending, in_progress, passed, failed, ready_for_manual)"
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Pending,
    InProgress,
    Passed,
    Failed,
    ReadyForManual,
}

impl fmt::Display for StepStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StepStatus::Pending => f.write_str("pending"),
            StepStatus::InProgress => f.write_str("in_progress"),
            StepStatus::Passed => f.write_str("passed"),
            StepStatus::Failed => f.write_str("failed"),
            StepStatus::ReadyForManual => f.write_str("ready_for_manual"),
        }
    }
}

impl FromStr for StepStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pending" => Ok(StepStatus::Pending),
            "in_progress" => Ok(StepStatus::InProgress),
            "passed" => Ok(StepStatus::Passed),
            "failed" => Ok(StepStatus::Failed),
            "ready_for_manual" => Ok(StepStatus::ReadyForManual),
            _ => Err(format!(
                "invalid step status '{s}' (expected one of: pending, in_progress, passed, failed, ready_for_manual)"
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckState {
    Pending,
    Passed,
    Failed,
}

impl CheckState {
    pub fn marker(self) -> &'static str {
        match self {
            CheckState::Pending => " ",
            CheckState::Passed => "x",
            CheckState::Failed => "!",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerifyMode {
    Auto,
    Manual,
}

impl fmt::Display for VerifyMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VerifyMode::Auto => f.write_str("auto"),
            VerifyMode::Manual => f.write_str("manual"),
        }
    }
}

impl FromStr for VerifyMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "auto" => Ok(VerifyMode::Auto),
            "manual" => Ok(VerifyMode::Manual),
            _ => Err(format!(
                "invalid verify mode '{s}' (expected 'auto' or 'manual')"
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScenarioKind {
    #[default]
    Scenario,
    Togaki,
}

impl fmt::Display for ScenarioKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScenarioKind::Scenario => f.write_str("scenario"),
            ScenarioKind::Togaki => f.write_str("togaki"),
        }
    }
}

impl FromStr for ScenarioKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "scenario" => Ok(ScenarioKind::Scenario),
            "togaki" => Ok(ScenarioKind::Togaki),
            _ => Err(format!(
                "invalid scenario kind '{s}' (expected 'scenario' or 'togaki')"
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrontMatter {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub kind: ScenarioKind,
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub attach_step: Option<usize>,
    pub priority: Priority,
    pub status: ScenarioStatus,
    pub last_checked_at: Option<String>,
    pub last_checked_by: Option<String>,
    pub notified_pass_at: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub hosts: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckItem {
    pub id: String,
    pub mode: VerifyMode,
    pub state: CheckState,
    pub description: String,
    pub reason: Option<String>,
    #[serde(skip)]
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    pub number: usize,
    pub title: String,
    pub status: StepStatus,
    pub failed_check: Option<String>,
    pub summary: Option<String>,
    pub checks: Vec<CheckItem>,
    #[serde(skip)]
    pub line: usize,
    #[serde(skip)]
    pub status_line: Option<usize>,
    #[serde(skip)]
    pub failed_check_line: Option<usize>,
}

impl Step {
    pub fn first_failed_check(&self) -> Option<&str> {
        self.checks
            .iter()
            .find(|c| c.state == CheckState::Failed)
            .map(|c| c.id.as_str())
    }

    pub fn has_failed_check(&self) -> bool {
        self.checks.iter().any(|c| c.state == CheckState::Failed)
    }

    pub fn all_checks_passed(&self) -> bool {
        !self.checks.is_empty() && self.checks.iter().all(|c| c.state == CheckState::Passed)
    }

    pub fn all_auto_passed(&self) -> bool {
        self.checks
            .iter()
            .filter(|c| c.mode == VerifyMode::Auto)
            .all(|c| c.state == CheckState::Passed)
    }

    pub fn has_auto_checks(&self) -> bool {
        self.checks.iter().any(|c| c.mode == VerifyMode::Auto)
    }

    pub fn has_pending_manual_checks(&self) -> bool {
        self.checks
            .iter()
            .any(|c| c.mode == VerifyMode::Manual && c.state == CheckState::Pending)
    }

    pub fn has_pending_auto_checks(&self) -> bool {
        self.checks
            .iter()
            .any(|c| c.mode == VerifyMode::Auto && c.state == CheckState::Pending)
    }

    pub fn has_any_passed(&self) -> bool {
        self.checks.iter().any(|c| c.state == CheckState::Passed)
    }

    /// Computes the canonical step status from its check items.
    pub fn derived_status(&self) -> StepStatus {
        if self.has_failed_check() {
            StepStatus::Failed
        } else if self.all_checks_passed() {
            StepStatus::Passed
        } else if self.has_auto_checks()
            && self.all_auto_passed()
            && self.has_pending_manual_checks()
        {
            StepStatus::ReadyForManual
        } else if self.has_any_passed() {
            StepStatus::InProgress
        } else {
            StepStatus::Pending
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scenario {
    pub front_matter: FrontMatter,
    pub overview: String,
    pub steps: Vec<Step>,
    /// Raw YAML lines between the `---` delimiters for surgical write-back.
    #[serde(skip)]
    pub raw_front_matter: String,
    /// Verbatim Markdown between the closing `---` and the end of the `## ステップ` heading line.
    #[serde(skip)]
    pub raw_before_steps: String,
    /// Verbatim Markdown for any trailing `## ...` sections after `## ステップ`.
    #[serde(skip)]
    pub raw_after_steps: String,
}

impl Scenario {
    pub fn is_togaki(&self) -> bool {
        self.front_matter.kind == ScenarioKind::Togaki
    }

    /// Returns `true` when this scenario has progressed enough to unlock child Togaki
    /// (all steps passed, or all automated steps passed awaiting manual check).
    pub fn unlocks_children(&self) -> bool {
        matches!(
            self.front_matter.status,
            ScenarioStatus::Passed | ScenarioStatus::ReadyForManual
        )
    }

    pub fn all_auto_checks_passed(&self) -> bool {
        self.steps.iter().all(Step::all_auto_passed)
    }

    pub fn has_any_auto_checks(&self) -> bool {
        self.steps.iter().any(Step::has_auto_checks)
    }

    pub fn has_pending_manual_checks(&self) -> bool {
        self.steps.iter().any(Step::has_pending_manual_checks)
    }

    pub fn derived_status(&self) -> ScenarioStatus {
        if self.steps.iter().any(|s| s.status == StepStatus::Failed) {
            return ScenarioStatus::Failed;
        }
        if !self.steps.is_empty() && self.steps.iter().all(|s| s.status == StepStatus::Passed) {
            return ScenarioStatus::Passed;
        }
        let all_auto_ok = self.all_auto_checks_passed();
        let any_ready_manual_step = self
            .steps
            .iter()
            .any(|s| s.status == StepStatus::ReadyForManual);
        if !self.steps.is_empty()
            && all_auto_ok
            && self.has_pending_manual_checks()
            && (self.has_any_auto_checks() || any_ready_manual_step)
        {
            return ScenarioStatus::ReadyForManual;
        }
        if self.steps.iter().any(|s| {
            s.has_any_passed()
                || matches!(s.status, StepStatus::InProgress | StepStatus::ReadyForManual)
        }) {
            return ScenarioStatus::InProgress;
        }
        ScenarioStatus::Pending
    }

    /// Recomputes step statuses, `failed_check`, and overall `front_matter.status` from check items.
    pub fn recompute_statuses(&mut self) {
        for step in &mut self.steps {
            step.status = step.derived_status();
            step.failed_check = if step.status == StepStatus::Failed {
                step.first_failed_check().map(ToOwned::to_owned)
            } else {
                None
            };
        }
        let all_auto_ok = self.all_auto_checks_passed();
        if !self.steps.iter().any(|s| s.status == StepStatus::Failed)
            && all_auto_ok
            && self.has_any_auto_checks()
        {
            for step in &mut self.steps {
                if !step.has_auto_checks() && step.has_pending_manual_checks() {
                    step.status = StepStatus::ReadyForManual;
                }
            }
        }
        self.front_matter.status = self.derived_status();
    }

    /// Renders a brand-new canonical Markdown scenario file from scratch.
    pub fn render_new_markdown(&self) -> String {
        let mut out = String::new();
        out.push_str("---\n");
        out.push_str(&format!("id: {}\n", self.front_matter.id));
        out.push_str(&format!("title: {}\n", self.front_matter.title));
        if self.front_matter.kind == ScenarioKind::Togaki {
            out.push_str("kind: togaki\n");
        }
        if let Some(ref parent) = self.front_matter.parent {
            out.push_str(&format!("parent: {parent}\n"));
        }
        if let Some(step_num) = self.front_matter.attach_step {
            out.push_str(&format!("attach_step: {step_num}\n"));
        }
        out.push_str(&format!("priority: {}\n", self.front_matter.priority));
        out.push_str(&format!("status: {}\n", self.front_matter.status));
        out.push_str(&format!(
            "last_checked_at: {}\n",
            format_optional_yaml(&self.front_matter.last_checked_at)
        ));
        out.push_str(&format!(
            "last_checked_by: {}\n",
            format_optional_yaml(&self.front_matter.last_checked_by)
        ));
        out.push_str(&format!(
            "notified_pass_at: {}\n",
            format_optional_yaml(&self.front_matter.notified_pass_at)
        ));
        if self.front_matter.tags.is_empty() {
            out.push_str("tags: []\n");
        } else {
            out.push_str("tags:\n");
            for tag in &self.front_matter.tags {
                out.push_str(&format!("  - {tag}\n"));
            }
        }
        if !self.front_matter.hosts.is_empty() {
            out.push_str("hosts:\n");
            for host in &self.front_matter.hosts {
                out.push_str(&format!("  - {host}\n"));
            }
        }
        out.push_str("---\n\n## 概要\n");
        out.push_str(self.overview.trim());
        out.push_str("\n\n## ステップ\n");
        out.push_str(&render_steps_body(&self.steps));
        out
    }
}

pub(crate) fn format_optional_yaml(value: &Option<String>) -> String {
    match value {
        Some(v) => v.clone(),
        None => "null".to_string(),
    }
}

pub(crate) fn render_steps_body(steps: &[Step]) -> String {
    let mut out = String::new();
    for step in steps {
        out.push('\n');
        out.push_str(&format!("### Step {}: {}\n", step.number, step.title));
        out.push_str(&format!("- status: {}\n", step.status));
        if let Some(ref failed) = step.failed_check {
            out.push_str(&format!("- failed_check: {failed}\n"));
        }
        if let Some(ref summary) = step.summary {
            out.push_str(&format!("- summary: {summary}\n"));
        }
        out.push('\n');
        for check in &step.checks {
            out.push_str(&format!(
                "- [{}] [{}] {}: {}\n",
                check.state.marker(),
                check.mode,
                check.id,
                check.description
            ));
            if let Some(ref reason) = check.reason {
                out.push_str(&format!("  - reason: {reason}\n"));
            }
        }
    }
    out
}
