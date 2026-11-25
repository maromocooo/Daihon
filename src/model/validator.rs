use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::model::parser::parse_scenario_str;
use crate::model::scenario::{CheckState, Scenario, ScenarioStatus, Step, StepStatus};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ValidationIssue {
    pub path: Option<PathBuf>,
    pub line: Option<usize>,
    pub message: String,
}

impl ValidationIssue {
    pub fn new(path: Option<&Path>, line: Option<usize>, message: impl Into<String>) -> Self {
        Self {
            path: path.map(Path::to_path_buf),
            line,
            message: message.into(),
        }
    }
}

impl fmt::Display for ValidationIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.path, self.line) {
            (Some(p), Some(l)) => write!(f, "{}:{}: {}", p.display(), l, self.message),
            (Some(p), None) => write!(f, "{}: {}", p.display(), self.message),
            (None, Some(l)) => write!(f, "line {}: {}", l, self.message),
            (None, None) => write!(f, "{}", self.message),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct FileValidationResult {
    pub path: PathBuf,
    pub scenario_id: Option<String>,
    pub valid: bool,
    pub issues: Vec<ValidationIssue>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ValidationReport {
    pub files: Vec<FileValidationResult>,
    pub cross_file_issues: Vec<ValidationIssue>,
}

impl ValidationReport {
    pub fn is_ok(&self) -> bool {
        self.cross_file_issues.is_empty() && self.files.iter().all(|f| f.valid)
    }

    pub fn total_issues(&self) -> usize {
        self.cross_file_issues.len() + self.files.iter().map(|f| f.issues.len()).sum::<usize>()
    }
}

/// Parses and semantically validates a single scenario Markdown string.
pub fn validate_str(
    content: &str,
    path: Option<&Path>,
) -> Result<Scenario, Vec<ValidationIssue>> {
    let (scenario, mut issues) = parse_scenario_str(content, path)?;
    validate_scenario_semantics(&scenario, path, &mut issues);

    if issues.is_empty() {
        Ok(scenario)
    } else {
        Err(issues)
    }
}

/// Reads and validates a single scenario file from disk.
pub fn validate_file(path: &Path) -> Result<Scenario, Vec<ValidationIssue>> {
    let content = fs::read_to_string(path).map_err(|err| {
        vec![ValidationIssue::new(
            Some(path),
            None,
            format!("ファイルを読み込めません: {err}"),
        )]
    })?;
    validate_str(&content, Some(path))
}

/// Validates either a single `.md` file or every `.md` file directly under a directory
/// (including cross-file duplicate `id` detection).
pub fn validate_path(path: &Path) -> ValidationReport {
    if path.is_file() {
        return match validate_file(path) {
            Ok(sc) => ValidationReport {
                files: vec![FileValidationResult {
                    path: path.to_path_buf(),
                    scenario_id: Some(sc.front_matter.id),
                    valid: true,
                    issues: Vec::new(),
                }],
                cross_file_issues: Vec::new(),
            },
            Err(issues) => ValidationReport {
                files: vec![FileValidationResult {
                    path: path.to_path_buf(),
                    scenario_id: None,
                    valid: false,
                    issues,
                }],
                cross_file_issues: Vec::new(),
            },
        };
    }

    if !path.is_dir() {
        return ValidationReport {
            files: Vec::new(),
            cross_file_issues: vec![ValidationIssue::new(
                Some(path),
                None,
                "指定されたパスが存在しません",
            )],
        };
    }

    let entries = match fs::read_dir(path) {
        Ok(rd) => rd,
        Err(err) => {
            return ValidationReport {
                files: Vec::new(),
                cross_file_issues: vec![ValidationIssue::new(
                    Some(path),
                    None,
                    format!("ディレクトリを読み込めません: {err}"),
                )],
            };
        }
    };

    let mut md_paths: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().and_then(|e| e.to_str()) == Some("md"))
        .collect();
    md_paths.sort();

    if md_paths.is_empty() {
        return ValidationReport {
            files: Vec::new(),
            cross_file_issues: vec![ValidationIssue::new(
                Some(path),
                None,
                "シナリオファイル（.md）が 1 件も見つかりません",
            )],
        };
    }

    let mut files = Vec::with_capacity(md_paths.len());
    let mut cross_file_issues = Vec::new();
    let mut seen_ids: HashMap<String, PathBuf> = HashMap::new();

    for file_path in md_paths {
        match validate_file(&file_path) {
            Ok(sc) => {
                let id = sc.front_matter.id;
                if let Some(prev_path) = seen_ids.get(&id) {
                    cross_file_issues.push(ValidationIssue::new(
                        Some(&file_path),
                        None,
                        format!(
                            "シナリオ `id` ('{id}') が {} と重複しています",
                            prev_path.display()
                        ),
                    ));
                } else {
                    seen_ids.insert(id.clone(), file_path.clone());
                }
                files.push(FileValidationResult {
                    path: file_path,
                    scenario_id: Some(id),
                    valid: true,
                    issues: Vec::new(),
                });
            }
            Err(issues) => {
                files.push(FileValidationResult {
                    path: file_path,
                    scenario_id: None,
                    valid: false,
                    issues,
                });
            }
        }
    }

    ValidationReport {
        files,
        cross_file_issues,
    }
}

fn validate_scenario_semantics(
    scenario: &Scenario,
    path: Option<&Path>,
    issues: &mut Vec<ValidationIssue>,
) {
    if scenario.overview.trim().is_empty() {
        issues.push(ValidationIssue::new(
            path,
            None,
            "`## 概要` の本文が空です（自然言語でシナリオの意図を記述してください）",
        ));
    }

    if scenario.steps.is_empty() {
        issues.push(ValidationIssue::new(
            path,
            None,
            "`## ステップ` 配下に `### Step 1: ...` が 1 つもありません",
        ));
        return;
    }

    let mut seen_check_ids: HashSet<&str> = HashSet::new();

    for (idx, step) in scenario.steps.iter().enumerate() {
        let expected_num = idx + 1;
        if step.number != expected_num {
            issues.push(ValidationIssue::new(
                path,
                Some(step.line),
                format!(
                    "ステップ番号が連番になっていません（期待: Step {expected_num}、実際: Step {}）",
                    step.number
                ),
            ));
        }

        if step.status_line.is_none() {
            issues.push(ValidationIssue::new(
                path,
                Some(step.line),
                format!("Step {} に必須メタデータ `- status: ...` がありません", step.number),
            ));
        }

        if step.checks.is_empty() {
            issues.push(ValidationIssue::new(
                path,
                Some(step.line),
                format!("Step {} にチェック項目が 1 つもありません", step.number),
            ));
            continue;
        }

        for check in &step.checks {
            if !seen_check_ids.insert(check.id.as_str()) {
                issues.push(ValidationIssue::new(
                    path,
                    Some(check.line),
                    format!("チェック項目 ID `{}` がシナリオ内で重複しています", check.id),
                ));
            }
        }

        validate_step_consistency(step, path, issues);
    }

    validate_scenario_status_consistency(scenario, path, issues);
}

fn validate_step_consistency(
    step: &Step,
    path: Option<&Path>,
    issues: &mut Vec<ValidationIssue>,
) {
    let status_line = step.status_line.unwrap_or(step.line);

    match step.status {
        StepStatus::Failed => {
            if !step.has_failed_check() {
                issues.push(ValidationIssue::new(
                    path,
                    Some(status_line),
                    format!(
                        "Step {} は `status: failed` ですが、`[!]` のチェック項目がありません",
                        step.number
                    ),
                ));
            }
            match &step.failed_check {
                None => issues.push(ValidationIssue::new(
                    path,
                    Some(status_line),
                    format!(
                        "Step {} は `status: failed` のため `- failed_check: <check-id>` が必須です",
                        step.number
                    ),
                )),
                Some(failed_id) => {
                    let matches_failed = step
                        .checks
                        .iter()
                        .any(|c| c.id == *failed_id && c.state == CheckState::Failed);
                    if !matches_failed {
                        issues.push(ValidationIssue::new(
                            path,
                            step.failed_check_line.or(Some(status_line)),
                            format!(
                                "Step {} の `failed_check: {failed_id}` が `[!]` のチェック項目と一致しません",
                                step.number
                            ),
                        ));
                    }
                }
            }
        }
        other => {
            if step.has_failed_check() {
                issues.push(ValidationIssue::new(
                    path,
                    Some(status_line),
                    format!(
                        "Step {} に `[!]` のチェック項目があるため、`status` は `failed` である必要があります（現在: {other}）",
                        step.number
                    ),
                ));
            }
            if step.failed_check.is_some() {
                issues.push(ValidationIssue::new(
                    path,
                    step.failed_check_line.or(Some(status_line)),
                    format!(
                        "Step {} は `status: {other}` なので `failed_check` は指定できません",
                        step.number
                    ),
                ));
            }

            match other {
                StepStatus::Passed => {
                    if !step.all_checks_passed() {
                        issues.push(ValidationIssue::new(
                            path,
                            Some(status_line),
                            format!(
                                "Step {} は `status: passed` ですが、未通過のチェック項目が残っています",
                                step.number
                            ),
                        ));
                    }
                }
                StepStatus::ReadyForManual => {
                    if !step.all_auto_passed() || !step.has_pending_manual_checks() {
                        issues.push(ValidationIssue::new(
                            path,
                            Some(status_line),
                            format!(
                                "Step {} の `status: ready_for_manual` は全 `[auto]` 項目が `[x]` かつ未確認 `[manual]` 項目がある場合のみ指定できます",
                                step.number
                            ),
                        ));
                    }
                }
                StepStatus::InProgress => {
                    if !step.has_any_passed() || !step.has_pending_auto_checks() {
                        issues.push(ValidationIssue::new(
                            path,
                            Some(status_line),
                            format!(
                                "Step {} の `status: in_progress` は通過済み項目と未検証 `[auto]` 項目の両方がある場合のみ指定できます",
                                step.number
                            ),
                        ));
                    }
                }
                StepStatus::Pending => {
                    if step.has_any_passed() {
                        issues.push(ValidationIssue::new(
                            path,
                            Some(status_line),
                            format!(
                                "Step {} は `status: pending` ですが、既に `[x]` のチェック項目があります",
                                step.number
                            ),
                        ));
                    }
                }
                StepStatus::Failed => unreachable!(),
            }
        }
    }
}

fn validate_scenario_status_consistency(
    scenario: &Scenario,
    path: Option<&Path>,
    issues: &mut Vec<ValidationIssue>,
) {
    let any_failed = scenario.steps.iter().any(|s| s.status == StepStatus::Failed);
    let all_passed = scenario.steps.iter().all(|s| s.status == StepStatus::Passed);
    let all_pending = scenario.steps.iter().all(|s| s.status == StepStatus::Pending);
    let all_auto_passed = scenario.all_auto_checks_passed();
    let has_pending_manual = scenario.has_pending_manual_checks();
    let has_pending_auto = scenario.steps.iter().any(Step::has_pending_auto_checks);
    let has_any_progress = scenario
        .steps
        .iter()
        .any(|s| s.has_any_passed() || s.status == StepStatus::InProgress);

    let ok = match scenario.front_matter.status {
        ScenarioStatus::Failed => any_failed,
        ScenarioStatus::Passed => !any_failed && all_passed,
        ScenarioStatus::ReadyForManual => {
            !any_failed && all_auto_passed && has_pending_manual
        }
        ScenarioStatus::InProgress => !any_failed && has_any_progress && has_pending_auto,
        ScenarioStatus::Pending => !any_failed && all_pending,
    };

    if !ok {
        issues.push(ValidationIssue::new(
            path,
            None,
            format!(
                "front matter の `status: {}` がステップ状態と矛盾しています（導出される状態: `{}`）",
                scenario.front_matter.status,
                scenario.derived_status()
            ),
        ));
    }
}
