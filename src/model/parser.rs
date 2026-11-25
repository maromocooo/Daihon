use std::path::Path;

use chrono::DateTime;
use serde_yml::Value;

use crate::model::scenario::{
    CheckItem, CheckState, FrontMatter, Priority, Scenario, ScenarioStatus, Step, StepStatus,
    VerifyMode, format_optional_yaml, render_steps_body,
};
use crate::model::validator::ValidationIssue;

/// Parses a scenario Markdown file, returning structured data plus any parse-level issues.
pub fn parse_scenario_str(
    input: &str,
    path: Option<&Path>,
) -> Result<(Scenario, Vec<ValidationIssue>), Vec<ValidationIssue>> {
    let normalized = input.replace("\r\n", "\n");
    let lines: Vec<&str> = normalized.split('\n').collect();
    let mut issues = Vec::new();

    // 1. Extract YAML front matter between leading `---` and closing `---`.
    if lines.first().map(|l| l.trim()) != Some("---") {
        issues.push(ValidationIssue::new(
            path,
            Some(1),
            "ファイル先頭は YAML front matter の開始行 `---` である必要があります",
        ));
        return Err(issues);
    }

    let closing_idx = lines
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_, line)| line.trim() == "---")
        .map(|(idx, _)| idx);

    let Some(closing_idx) = closing_idx else {
        issues.push(ValidationIssue::new(
            path,
            Some(1),
            "YAML front matter の終了行 `---` が見つかりません",
        ));
        return Err(issues);
    };

    let raw_front_matter = lines[1..closing_idx].join("\n");
    let front_matter = match parse_front_matter(&raw_front_matter, path, &mut issues) {
        Some(fm) => fm,
        None => return Err(issues),
    };

    // 2. Locate `## 概要` and `## ステップ` sections outside fenced code blocks.
    let body_start_idx = closing_idx + 1;
    let mut in_code_fence = false;
    let mut overview_heading_idx: Option<usize> = None;
    let mut overview_end_idx: Option<usize> = None;
    let mut steps_heading_idx: Option<usize> = None;
    let mut steps_end_idx: Option<usize> = None;

    for (idx, line) in lines.iter().enumerate().skip(body_start_idx) {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            in_code_fence = !in_code_fence;
            continue;
        }
        if in_code_fence {
            continue;
        }

        if let Some(heading) = trimmed.strip_prefix("## ") {
            let heading = heading.trim();
            if overview_heading_idx.is_some() && overview_end_idx.is_none() {
                overview_end_idx = Some(idx);
            }
            if steps_heading_idx.is_some() && steps_end_idx.is_none() {
                steps_end_idx = Some(idx);
            }

            if heading == "概要" {
                if overview_heading_idx.is_some() {
                    issues.push(ValidationIssue::new(
                        path,
                        Some(idx + 1),
                        "`## 概要` セクションが重複しています",
                    ));
                } else {
                    overview_heading_idx = Some(idx);
                }
            } else if heading == "ステップ" {
                if steps_heading_idx.is_some() {
                    issues.push(ValidationIssue::new(
                        path,
                        Some(idx + 1),
                        "`## ステップ` セクションが重複しています",
                    ));
                } else {
                    steps_heading_idx = Some(idx);
                }
            }
        }
    }

    if overview_heading_idx.is_none() {
        issues.push(ValidationIssue::new(
            path,
            None,
            "必須セクション `## 概要` が見つかりません",
        ));
    }

    let Some(steps_idx) = steps_heading_idx else {
        issues.push(ValidationIssue::new(
            path,
            None,
            "必須セクション `## ステップ` が見つかりません",
        ));
        return Err(issues);
    };

    if let Some(ov_idx) = overview_heading_idx
        && steps_idx < ov_idx
    {
        issues.push(ValidationIssue::new(
            path,
            Some(steps_idx + 1),
            "`## ステップ` は `## 概要` の後に配置してください",
        ));
    }

    let overview = if let Some(ov_idx) = overview_heading_idx {
        let end = overview_end_idx.unwrap_or(lines.len());
        lines[(ov_idx + 1)..end].join("\n").trim().to_string()
    } else {
        String::new()
    };

    let raw_before_steps = lines[body_start_idx..=steps_idx].join("\n");
    let steps_end = steps_end_idx.unwrap_or(lines.len());
    let raw_after_steps = if steps_end < lines.len() {
        lines[steps_end..].join("\n")
    } else {
        String::new()
    };

    // 3. Parse steps and check items inside `## ステップ`.
    let steps = parse_steps(&lines[(steps_idx + 1)..steps_end], steps_idx + 2, path, &mut issues);

    let scenario = Scenario {
        front_matter,
        overview,
        steps,
        raw_front_matter,
        raw_before_steps,
        raw_after_steps,
    };

    Ok((scenario, issues))
}

fn parse_front_matter(
    raw: &str,
    path: Option<&Path>,
    issues: &mut Vec<ValidationIssue>,
) -> Option<FrontMatter> {
    let value: Value = match serde_yml::from_str(raw) {
        Ok(v) => v,
        Err(err) => {
            issues.push(ValidationIssue::new(
                path,
                Some(2),
                format!("YAML front matter の構文エラー: {err}"),
            ));
            return None;
        }
    };

    let Some(map) = value.as_mapping() else {
        issues.push(ValidationIssue::new(
            path,
            Some(2),
            "YAML front matter はキーと値のマッピングである必要があります",
        ));
        return None;
    };

    let get_str = |key: &str| -> Option<String> {
        map.get(Value::String(key.to_string())).and_then(|v| match v {
            Value::String(s) => Some(s.clone()),
            _ => None,
        })
    };

    let id = match map.get(Value::String("id".to_string())) {
        Some(Value::String(s)) if !s.trim().is_empty() => {
            if !is_valid_kebab_id(s) {
                issues.push(ValidationIssue::new(
                    path,
                    find_front_matter_line(raw, "id"),
                    format!(
                        "`id` ('{s}') は小文字英数字とハイフンの kebab-case（例: rio-herdr-cross-machine）で記述してください"
                    ),
                ));
            }
            s.clone()
        }
        Some(_) => {
            issues.push(ValidationIssue::new(
                path,
                find_front_matter_line(raw, "id"),
                "front matter の `id` は非空の文字列である必要があります",
            ));
            String::new()
        }
        None => {
            issues.push(ValidationIssue::new(
                path,
                Some(2),
                "front matter に必須項目 `id` がありません",
            ));
            String::new()
        }
    };

    let title = match map.get(Value::String("title".to_string())) {
        Some(Value::String(s)) if !s.trim().is_empty() => s.clone(),
        Some(_) => {
            issues.push(ValidationIssue::new(
                path,
                find_front_matter_line(raw, "title"),
                "front matter の `title` は非空の文字列である必要があります",
            ));
            String::new()
        }
        None => {
            issues.push(ValidationIssue::new(
                path,
                Some(2),
                "front matter に必須項目 `title` がありません",
            ));
            String::new()
        }
    };

    let priority = match get_str("priority") {
        Some(s) => match s.parse::<Priority>() {
            Ok(p) => p,
            Err(msg) => {
                issues.push(ValidationIssue::new(
                    path,
                    find_front_matter_line(raw, "priority"),
                    format!("front matter の `priority` が不正です: {msg}"),
                ));
                Priority::P3
            }
        },
        None => {
            issues.push(ValidationIssue::new(
                path,
                find_front_matter_line(raw, "priority").or(Some(2)),
                "front matter に必須項目 `priority`（P0〜P3）がありません",
            ));
            Priority::P3
        }
    };

    let status = match get_str("status") {
        Some(s) => match s.parse::<ScenarioStatus>() {
            Ok(st) => st,
            Err(msg) => {
                issues.push(ValidationIssue::new(
                    path,
                    find_front_matter_line(raw, "status"),
                    format!("front matter の `status` が不正です: {msg}"),
                ));
                ScenarioStatus::Pending
            }
        },
        None => {
            issues.push(ValidationIssue::new(
                path,
                find_front_matter_line(raw, "status").or(Some(2)),
                "front matter に必須項目 `status` がありません",
            ));
            ScenarioStatus::Pending
        }
    };

    let last_checked_at = parse_optional_rfc3339(map, raw, "last_checked_at", path, issues);
    let last_checked_by = parse_optional_non_empty_str(map, raw, "last_checked_by", path, issues);
    let notified_pass_at = parse_optional_rfc3339(map, raw, "notified_pass_at", path, issues);
    let tags = parse_optional_str_list(map, raw, "tags", path, issues);
    let hosts = parse_optional_str_list(map, raw, "hosts", path, issues);

    Some(FrontMatter {
        id,
        title,
        priority,
        status,
        last_checked_at,
        last_checked_by,
        notified_pass_at,
        tags,
        hosts,
    })
}

fn parse_optional_rfc3339(
    map: &serde_yml::Mapping,
    raw: &str,
    key: &str,
    path: Option<&Path>,
    issues: &mut Vec<ValidationIssue>,
) -> Option<String> {
    match map.get(Value::String(key.to_string())) {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => {
            if DateTime::parse_from_rfc3339(s).is_err() {
                issues.push(ValidationIssue::new(
                    path,
                    find_front_matter_line(raw, key),
                    format!(
                        "front matter の `{key}` ('{s}') は RFC 3339 形式（例: 2026-11-25T09:30:00+09:00）で記述してください"
                    ),
                ));
            }
            Some(s.clone())
        }
        Some(_) => {
            issues.push(ValidationIssue::new(
                path,
                find_front_matter_line(raw, key),
                format!("front matter の `{key}` は null または日時文字列である必要があります"),
            ));
            None
        }
    }
}

fn parse_optional_non_empty_str(
    map: &serde_yml::Mapping,
    raw: &str,
    key: &str,
    path: Option<&Path>,
    issues: &mut Vec<ValidationIssue>,
) -> Option<String> {
    match map.get(Value::String(key.to_string())) {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) if !s.trim().is_empty() => Some(s.clone()),
        Some(_) => {
            issues.push(ValidationIssue::new(
                path,
                find_front_matter_line(raw, key),
                format!("front matter の `{key}` は null または非空文字列である必要があります"),
            ));
            None
        }
    }
}

fn parse_optional_str_list(
    map: &serde_yml::Mapping,
    raw: &str,
    key: &str,
    path: Option<&Path>,
    issues: &mut Vec<ValidationIssue>,
) -> Vec<String> {
    match map.get(Value::String(key.to_string())) {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Sequence(seq)) => {
            let mut items = Vec::new();
            for item in seq {
                match item {
                    Value::String(s) if !s.trim().is_empty() => items.push(s.clone()),
                    _ => issues.push(ValidationIssue::new(
                        path,
                        find_front_matter_line(raw, key),
                        format!("front matter の `{key}` の各要素は非空文字列である必要があります"),
                    )),
                }
            }
            items
        }
        Some(_) => {
            issues.push(ValidationIssue::new(
                path,
                find_front_matter_line(raw, key),
                format!("front matter の `{key}` は文字列の配列である必要があります"),
            ));
            Vec::new()
        }
    }
}

fn find_front_matter_line(raw: &str, key: &str) -> Option<usize> {
    let prefix = format!("{key}:");
    raw.lines()
        .enumerate()
        .find(|(_, line)| line.starts_with(&prefix))
        .map(|(idx, _)| idx + 2)
}

fn parse_steps(
    section_lines: &[&str],
    start_line_no: usize,
    path: Option<&Path>,
    issues: &mut Vec<ValidationIssue>,
) -> Vec<Step> {
    let mut steps: Vec<Step> = Vec::new();
    let mut current: Option<Step> = None;
    let mut saw_check_in_current = false;

    for (offset, raw_line) in section_lines.iter().enumerate() {
        let line_no = start_line_no + offset;
        let trimmed = raw_line.trim();

        if trimmed.is_empty() {
            continue;
        }

        if let Some(heading) = trimmed.strip_prefix("### ") {
            if let Some(step) = current.take() {
                steps.push(step);
            }
            saw_check_in_current = false;

            match parse_step_heading(heading.trim()) {
                Ok((number, title)) => {
                    current = Some(Step {
                        number,
                        title,
                        status: StepStatus::Pending,
                        failed_check: None,
                        summary: None,
                        checks: Vec::new(),
                        line: line_no,
                        status_line: None,
                        failed_check_line: None,
                    });
                }
                Err(msg) => {
                    issues.push(ValidationIssue::new(path, Some(line_no), msg));
                    current = None;
                }
            }
            continue;
        }

        let Some(step) = current.as_mut() else {
            issues.push(ValidationIssue::new(
                path,
                Some(line_no),
                format!(
                    "`### Step <N>: <タイトル>` の見出しより前に解釈できない行があります: {trimmed}"
                ),
            ));
            continue;
        };

        // Indented sub-bullet: `  - reason: ...`
        if (raw_line.starts_with("  ") || raw_line.starts_with('\t'))
            && let Some(rest) = trimmed.strip_prefix("- ")
        {
            if let Some(reason_val) = rest.strip_prefix("reason:") {
                let reason_text = reason_val.trim();
                if reason_text.is_empty() {
                    issues.push(ValidationIssue::new(
                        path,
                        Some(line_no),
                        "`reason:` の説明が空です",
                    ));
                } else if let Some(last_check) = step.checks.last_mut() {
                    if last_check.reason.is_some() {
                        issues.push(ValidationIssue::new(
                            path,
                            Some(line_no),
                            format!("チェック項目 `{}` に `reason:` が重複しています", last_check.id),
                        ));
                    } else {
                        last_check.reason = Some(reason_text.to_string());
                    }
                } else {
                    issues.push(ValidationIssue::new(
                        path,
                        Some(line_no),
                        "チェック項目の前に `reason:` が書かれています",
                    ));
                }
            } else {
                issues.push(ValidationIssue::new(
                    path,
                    Some(line_no),
                    format!("チェック項目配下で未対応のサブ項目です（`  - reason: ...` のみ対応）: {trimmed}"),
                ));
            }
            continue;
        }

        // Check item line: `- [<state>] [<mode>] <id>: <description>`
        if trimmed.starts_with("- [") {
            saw_check_in_current = true;
            match parse_check_item(trimmed, line_no) {
                Ok(item) => step.checks.push(item),
                Err(msg) => issues.push(ValidationIssue::new(path, Some(line_no), msg)),
            }
            continue;
        }

        // Step metadata line: `- status: ...`, `- failed_check: ...`, `- summary: ...`
        if let Some(meta) = trimmed.strip_prefix("- ") {
            if saw_check_in_current {
                issues.push(ValidationIssue::new(
                    path,
                    Some(line_no),
                    format!("チェック項目の後にステップメタデータまたは不正な箇条書きがあります: {trimmed}"),
                ));
                continue;
            }

            if let Some(val) = meta.strip_prefix("status:") {
                if step.status_line.is_some() {
                    issues.push(ValidationIssue::new(
                        path,
                        Some(line_no),
                        format!("Step {} に `status` が重複しています", step.number),
                    ));
                } else {
                    match val.trim().parse::<StepStatus>() {
                        Ok(st) => {
                            step.status = st;
                            step.status_line = Some(line_no);
                        }
                        Err(msg) => issues.push(ValidationIssue::new(
                            path,
                            Some(line_no),
                            format!("Step {} の `status` が不正です: {msg}", step.number),
                        )),
                    }
                }
            } else if let Some(val) = meta.strip_prefix("failed_check:") {
                let id = val.trim();
                if id.is_empty() {
                    issues.push(ValidationIssue::new(
                        path,
                        Some(line_no),
                        "`failed_check:` の値が空です",
                    ));
                } else if step.failed_check.is_some() {
                    issues.push(ValidationIssue::new(
                        path,
                        Some(line_no),
                        format!("Step {} に `failed_check` が重複しています", step.number),
                    ));
                } else {
                    step.failed_check = Some(id.to_string());
                    step.failed_check_line = Some(line_no);
                }
            } else if let Some(val) = meta.strip_prefix("summary:") {
                let text = val.trim();
                if text.is_empty() {
                    issues.push(ValidationIssue::new(
                        path,
                        Some(line_no),
                        "`summary:` の値が空です",
                    ));
                } else if step.summary.is_some() {
                    issues.push(ValidationIssue::new(
                        path,
                        Some(line_no),
                        format!("Step {} に `summary` が重複しています", step.number),
                    ));
                } else {
                    step.summary = Some(text.to_string());
                }
            } else {
                issues.push(ValidationIssue::new(
                    path,
                    Some(line_no),
                    format!(
                        "Step {} に未対応のメタデータ行があります（status / failed_check / summary のみ対応）: {trimmed}",
                        step.number
                    ),
                ));
            }
            continue;
        }

        issues.push(ValidationIssue::new(
            path,
            Some(line_no),
            format!("Step {} 内に解釈できない行があります: {trimmed}", step.number),
        ));
    }

    if let Some(step) = current {
        steps.push(step);
    }

    steps
}

fn parse_step_heading(heading: &str) -> Result<(usize, String), String> {
    let Some(rest) = heading.strip_prefix("Step ") else {
        return Err(format!(
            "ステップ見出しは `### Step <N>: <タイトル>` 形式で書いてください（実際: `### {heading}`）"
        ));
    };

    let Some((num_str, title_str)) = rest.split_once(':') else {
        return Err(format!(
            "ステップ見出しに `:` がありません（期待: `### Step <N>: <タイトル>`、実際: `### {heading}`）"
        ));
    };

    let number = num_str.trim().parse::<usize>().map_err(|_| {
        format!(
            "ステップ番号 '{}' が正の整数ではありません（実際: `### {heading}`）",
            num_str.trim()
        )
    })?;

    if number == 0 {
        return Err("ステップ番号は 1 から始めてください".to_string());
    }

    let title = title_str.trim();
    if title.is_empty() {
        return Err(format!("Step {number} のタイトルが空です"));
    }

    Ok((number, title.to_string()))
}

fn parse_check_item(line: &str, line_no: usize) -> Result<CheckItem, String> {
    let rest = line
        .strip_prefix("- [")
        .ok_or_else(|| "チェック項目は `- [ ]` / `- [x]` / `- [!]` で始めてください".to_string())?;

    let (state_str, after_state) = rest.split_once("] ").ok_or_else(|| {
        format!("チェック状態の閉じ括弧 `] ` が見つかりません: {line}")
    })?;

    let state = match state_str {
        " " => CheckState::Pending,
        "x" => CheckState::Passed,
        "!" => CheckState::Failed,
        other => {
            return Err(format!(
                "チェック状態 `[{other}]` は不正です（`[ ]`, `[x]`, `[!]` のいずれかを使用してください）"
            ));
        }
    };

    let after_mode_open = after_state.trim_start().strip_prefix('[').ok_or_else(|| {
        format!("チェック項目に検証区分 `[auto]` または `[manual]` がありません: {line}")
    })?;

    let (mode_str, after_mode) = after_mode_open.split_once(']').ok_or_else(|| {
        format!("検証区分の閉じ括弧 `]` が見つかりません: {line}")
    })?;

    let mode = mode_str
        .trim()
        .parse::<VerifyMode>()
        .map_err(|e| format!("検証区分が不正です: {e}"))?;

    let after_mode = after_mode.trim_start();
    let (id_part, desc_part) = after_mode.split_once(':').ok_or_else(|| {
        format!("チェック項目は `<check-id>: <説明>` 形式で記述してください: {line}")
    })?;

    let id = id_part.trim();
    if !is_valid_check_id(id) {
        return Err(format!(
            "チェック項目 ID '{id}' が不正です（英数字で始まり英数字・`-`・`_` のみ使用可）"
        ));
    }

    let description = desc_part.trim();
    if description.is_empty() {
        return Err(format!("チェック項目 `{id}` の説明が空です"));
    }

    Ok(CheckItem {
        id: id.to_string(),
        mode,
        state,
        description: description.to_string(),
        reason: None,
        line: line_no,
    })
}

pub(crate) fn is_valid_kebab_id(s: &str) -> bool {
    !s.is_empty()
        && s.split('-').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}

pub(crate) fn is_valid_check_id(s: &str) -> bool {
    let mut bytes = s.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    first.is_ascii_alphanumeric()
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Surgically writes back updated statuses into an existing parsed Scenario,
/// preserving unknown front matter keys/comments, `## 概要`, and extra sections byte-for-byte.
pub fn update_scenario_markdown(scenario: &Scenario) -> String {
    if scenario.raw_front_matter.is_empty() && scenario.raw_before_steps.is_empty() {
        return scenario.render_new_markdown();
    }

    let updated_fm = update_front_matter_lines(&scenario.raw_front_matter, &scenario.front_matter);
    let mut out = String::new();
    out.push_str("---\n");
    out.push_str(&updated_fm);
    if !updated_fm.ends_with('\n') {
        out.push('\n');
    }
    out.push_str("---\n");
    out.push_str(&scenario.raw_before_steps);
    if !scenario.raw_before_steps.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&render_steps_body(&scenario.steps));
    if !scenario.raw_after_steps.is_empty() {
        out.push('\n');
        out.push_str(&scenario.raw_after_steps);
        if !scenario.raw_after_steps.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

fn update_front_matter_lines(raw_fm: &str, fm: &FrontMatter) -> String {
    let updates: [(&str, String); 4] = [
        ("status", fm.status.to_string()),
        (
            "last_checked_at",
            format_optional_yaml(&fm.last_checked_at),
        ),
        (
            "last_checked_by",
            format_optional_yaml(&fm.last_checked_by),
        ),
        (
            "notified_pass_at",
            format_optional_yaml(&fm.notified_pass_at),
        ),
    ];

    let mut seen = [false; 4];
    let mut lines: Vec<String> = Vec::new();

    for line in raw_fm.lines() {
        let mut replaced = false;
        for (idx, (key, value)) in updates.iter().enumerate() {
            let prefix = format!("{key}:");
            if line.starts_with(&prefix) {
                lines.push(format!("{key}: {value}"));
                seen[idx] = true;
                replaced = true;
                break;
            }
        }
        if !replaced {
            lines.push(line.to_string());
        }
    }

    for (idx, (key, value)) in updates.iter().enumerate() {
        if !seen[idx] && (*key == "status" || value != "null") {
            lines.push(format!("{key}: {value}"));
        }
    }

    lines.join("\n")
}
