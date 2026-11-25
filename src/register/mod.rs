use std::collections::{BTreeMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::model::scenario::{
    CheckItem, CheckState, FrontMatter, Priority, Scenario, ScenarioStatus, Step, StepStatus,
    VerifyMode,
};
use crate::model::store::{LoadedScenario, load_scenarios_from_dir};
use crate::model::validator::{validate_file, validate_str};
use crate::runner::{AgentRequest, AgentRunner, ExecutionMode};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OverlapFinding {
    pub existing_id: String,
    pub existing_title: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PriorityProposal {
    pub suggested: Priority,
    pub rationale: String,
}

#[derive(Debug, Clone)]
pub struct RegistrationProposal {
    pub scenario: Scenario,
    pub overlaps: Vec<OverlapFinding>,
    pub priority_proposal: PriorityProposal,
}

#[derive(Debug, Deserialize)]
struct DraftPayload {
    id: String,
    title: String,
    priority: String,
    priority_rationale: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    hosts: Vec<String>,
    overview: String,
    steps: Vec<DraftStep>,
    #[serde(default)]
    overlaps: Vec<DraftOverlap>,
}

#[derive(Debug, Deserialize)]
struct DraftStep {
    title: String,
    checks: Vec<DraftCheck>,
}

#[derive(Debug, Deserialize)]
struct DraftCheck {
    #[serde(default)]
    id: String,
    mode: String,
    description: String,
}

#[derive(Debug, Deserialize)]
struct DraftOverlap {
    existing_id: String,
    reason: String,
}

pub fn registration_json_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["id", "title", "priority", "priority_rationale", "tags", "overview", "steps", "overlaps"],
        "properties": {
            "id": { "type": "string", "description": "kebab-case ASCII scenario identifier" },
            "title": { "type": "string", "description": "Short human-readable Japanese scenario title" },
            "priority": { "type": "string", "enum": ["P0", "P1", "P2", "P3"] },
            "priority_rationale": { "type": "string" },
            "tags": { "type": "array", "items": { "type": "string" } },
            "overview": { "type": "string", "description": "Natural language description of user intent" },
            "steps": {
                "type": "array",
                "minItems": 1,
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["title", "checks"],
                    "properties": {
                        "title": { "type": "string" },
                        "checks": {
                            "type": "array",
                            "minItems": 1,
                            "items": {
                                "type": "object",
                                "additionalProperties": false,
                                "required": ["id", "mode", "description"],
                                "properties": {
                                    "id": { "type": "string" },
                                    "mode": { "type": "string", "enum": ["auto", "manual"] },
                                    "description": { "type": "string" }
                                }
                            }
                        }
                    }
                }
            },
            "overlaps": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["existing_id", "reason"],
                    "properties": {
                        "existing_id": { "type": "string" },
                        "reason": { "type": "string" }
                    }
                }
            }
        }
    })
}

/// Converts a natural language scenario description into a validated proposal,
/// returning detected overlaps with existing scenarios and a priority suggestion.
pub fn propose_registration(
    natural_input: &str,
    scenarios_dir: &Path,
    runner: &dyn AgentRunner,
    workdir: &Path,
    timeout_secs: u64,
) -> Result<RegistrationProposal> {
    if natural_input.trim().is_empty() {
        bail!("シナリオの説明が空です。自然言語でやりたい使い方を入力してください");
    }

    let existing = load_scenarios_from_dir(scenarios_dir)?;
    let prompt = build_registration_prompt(natural_input, &existing);

    let response = runner.run(&AgentRequest {
        working_dir: workdir.to_path_buf(),
        prompt,
        json_schema: Some(registration_json_schema()),
        mode: ExecutionMode::ReadOnly,
        sandbox_override: None,
        env_vars: BTreeMap::new(),
        timeout_secs,
    })?;

    let json_val = response
        .structured_json
        .ok_or_else(|| anyhow!("登録エージェントから構造化 JSON が返りませんでした"))?;

    let preview: String = json_val.to_string().chars().take(300).collect();
    let draft: DraftPayload = serde_json::from_value(json_val).with_context(|| {
        format!("登録エージェントの JSON 応答が仕様と一致しません（受信内容: {preview}）")
    })?;

    if draft.steps.is_empty() {
        bail!("登録エージェントがステップを 1 つも生成しませんでした");
    }

    let existing_ids: HashSet<&str> = existing
        .iter()
        .map(|s| s.scenario.front_matter.id.as_str())
        .collect();
    let unique_id = ensure_unique_kebab_id(&draft.id, &existing_ids);

    let priority = draft.priority.parse::<Priority>().unwrap_or(Priority::P1);

    let mut steps = Vec::with_capacity(draft.steps.len());
    let mut used_check_ids = HashSet::new();

    for (idx, dstep) in draft.steps.into_iter().enumerate() {
        let step_num = idx + 1;
        if dstep.checks.is_empty() {
            bail!("Step {step_num} にチェック項目がありません");
        }
        let mut checks = Vec::with_capacity(dstep.checks.len());
        for (cidx, dcheck) in dstep.checks.into_iter().enumerate() {
            let mode = dcheck.mode.parse::<VerifyMode>().unwrap_or(VerifyMode::Auto);
            let fallback_id = format!("s{step_num}-c{}", cidx + 1);
            let mut cid = sanitize_check_id(&dcheck.id, &fallback_id);
            while !used_check_ids.insert(cid.clone()) {
                cid = format!("{fallback_id}-{}", used_check_ids.len() + 1);
            }
            checks.push(CheckItem {
                id: cid,
                mode,
                state: CheckState::Pending,
                description: dcheck.description.trim().to_string(),
                reason: None,
                line: 0,
            });
        }

        steps.push(Step {
            number: step_num,
            title: dstep.title.trim().to_string(),
            status: StepStatus::Pending,
            failed_check: None,
            summary: None,
            checks,
            line: 0,
            status_line: None,
            failed_check_line: None,
        });
    }

    let tags = draft
        .tags
        .into_iter()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect();

    let hosts = draft
        .hosts
        .into_iter()
        .map(|h| h.trim().to_string())
        .filter(|h| !h.is_empty())
        .collect();

    let scenario = Scenario {
        front_matter: FrontMatter {
            id: unique_id,
            title: draft.title.trim().to_string(),
            priority,
            status: ScenarioStatus::Pending,
            last_checked_at: None,
            last_checked_by: None,
            notified_pass_at: None,
            tags,
            hosts,
        },
        overview: draft.overview.trim().to_string(),
        steps,
        raw_front_matter: String::new(),
        raw_before_steps: String::new(),
        raw_after_steps: String::new(),
    };

    // Validate generated Markdown before showing it to the user.
    let rendered = scenario.render_new_markdown();
    if let Err(issues) = validate_str(&rendered, None) {
        let details = issues
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n  ");
        bail!("生成されたシナリオが形式チェックを通りませんでした:\n  {details}");
    }

    let mut overlaps = Vec::new();
    let mut reported_ids = HashSet::new();
    for item in draft.overlaps {
        if let Some(found) = existing
            .iter()
            .find(|s| s.scenario.front_matter.id == item.existing_id)
            && reported_ids.insert(item.existing_id.clone())
        {
            overlaps.push(OverlapFinding {
                existing_id: item.existing_id,
                existing_title: found.scenario.front_matter.title.clone(),
                reason: item.reason,
            });
        }
    }

    // Supplement with deterministic tag/keyword overlap detection if missed by the LLM.
    for ex in &existing {
        let ex_id = &ex.scenario.front_matter.id;
        if reported_ids.contains(ex_id) {
            continue;
        }
        if let Some(reason) = detect_lexical_overlap(&scenario, &ex.scenario) {
            reported_ids.insert(ex_id.clone());
            overlaps.push(OverlapFinding {
                existing_id: ex_id.clone(),
                existing_title: ex.scenario.front_matter.title.clone(),
                reason,
            });
        }
    }

    Ok(RegistrationProposal {
        scenario,
        overlaps,
        priority_proposal: PriorityProposal {
            suggested: priority,
            rationale: draft.priority_rationale.trim().to_string(),
        },
    })
}

/// Saves a confirmed registration proposal into a brand-new file in `scenarios_dir`.
/// Never modifies or overwrites any existing scenario file.
pub fn save_registration(
    proposal: &RegistrationProposal,
    scenarios_dir: &Path,
    override_priority: Option<Priority>,
) -> Result<PathBuf> {
    fs::create_dir_all(scenarios_dir).with_context(|| {
        format!(
            "シナリオ保存先ディレクトリを作成できません: {}",
            scenarios_dir.display()
        )
    })?;

    let mut scenario = proposal.scenario.clone();
    if let Some(p) = override_priority {
        scenario.front_matter.priority = p;
    }

    let next_num = next_file_prefix(scenarios_dir)?;
    let filename = format!("{next_num:02}-{}.md", scenario.front_matter.id);
    let target_path = scenarios_dir.join(filename);

    let markdown = scenario.render_new_markdown();
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&target_path)
        .with_context(|| {
            format!(
                "既存ファイルを上書きしないため新規作成に失敗しました: {}",
                target_path.display()
            )
        })?;
    file.write_all(markdown.as_bytes())?;
    file.flush()?;

    if let Err(issues) = validate_file(&target_path) {
        let _ = fs::remove_file(&target_path);
        let details = issues
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n  ");
        bail!("保存したファイルの形式検証に失敗しました:\n  {details}");
    }

    Ok(target_path)
}

fn build_registration_prompt(natural_input: &str, existing: &[LoadedScenario]) -> String {
    let existing_summary = if existing.is_empty() {
        "（既存シナリオはまだありません）".to_string()
    } else {
        existing
            .iter()
            .map(|l| {
                let sc = &l.scenario;
                let step_titles: Vec<&str> = sc.steps.iter().map(|s| s.title.as_str()).collect();
                format!(
                    "- id: {} | priority: {} | title: {}\n  tags: [{}]\n  steps: {}",
                    sc.front_matter.id,
                    sc.front_matter.priority,
                    sc.front_matter.title,
                    sc.front_matter.tags.join(", "),
                    step_titles.join(" / ")
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    format!(
        r#"あなたはシナリオ駆動の開発ハーネスの「シナリオ登録役」です。
ユーザーの自然言語による要望を、開発エージェントと検証エージェント（見張り役）が共有できる
明確なシナリオへ分解し、指定された JSON Schema に厳密に従って返してください。

ルール:
1. id は小文字英数字とハイフンの kebab-case（例: rio-herdr-cross-machine）にする。
2. overview には、ユーザーが本当に試したい使い方・目的を自然な日本語で残す。
3. steps は利用の流れに沿った順序に分け、各ステップの checks に具体的な OK 条件を書く。
4. チェック項目はコマンド・テスト・SSH 等で自動検証できるものを `auto`、実機 GUI 表示や操作感など人の確認が必要なものを `manual` に分類する。
5. チェック ID は `s1-c1`, `s1-c2`, `s2-c1` のように一意にする。
6. 既存シナリオと重なる部分があれば `overlaps` に該当 `existing_id` と重なり理由を挙げる。
7. 既存シナリオとの相対的な重要度から `priority`（P0〜P3）と `priority_rationale` を提案する。

## 既存シナリオ一覧
{existing_summary}

## ユーザーの新しいシナリオ要望
{natural_input}
"#
    )
}

fn ensure_unique_kebab_id(raw: &str, existing_ids: &HashSet<&str>) -> String {
    let mut base = String::new();
    let mut prev_dash = true;
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            base.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            base.push('-');
            prev_dash = true;
        }
    }
    let base = base.trim_matches('-').to_string();
    let base = if base.is_empty() {
        "new-scenario".to_string()
    } else {
        base
    };

    if !existing_ids.contains(base.as_str()) {
        return base;
    }

    let mut suffix = 2usize;
    loop {
        let candidate = format!("{base}-{suffix}");
        if !existing_ids.contains(candidate.as_str()) {
            return candidate;
        }
        suffix += 1;
    }
}

fn sanitize_check_id(raw: &str, fallback: &str) -> String {
    let cleaned: String = raw
        .trim()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    if cleaned.is_empty() || !cleaned.as_bytes()[0].is_ascii_alphanumeric() {
        fallback.to_string()
    } else {
        cleaned
    }
}

fn detect_lexical_overlap(candidate: &Scenario, existing: &Scenario) -> Option<String> {
    let cand_tags: HashSet<String> = candidate
        .front_matter
        .tags
        .iter()
        .map(|t| t.to_ascii_lowercase())
        .collect();
    let shared_tags: Vec<String> = existing
        .front_matter
        .tags
        .iter()
        .filter(|t| cand_tags.contains(&t.to_ascii_lowercase()))
        .cloned()
        .collect();

    if shared_tags.len() >= 2 {
        return Some(format!(
            "タグ（{}）が共通しています",
            shared_tags.join(", ")
        ));
    }

    let cand_tokens = extract_ascii_tokens(&format!(
        "{} {} {}",
        candidate.front_matter.id, candidate.front_matter.title, candidate.overview
    ));
    let ex_tokens = extract_ascii_tokens(&format!(
        "{} {} {}",
        existing.front_matter.id, existing.front_matter.title, existing.overview
    ));
    let shared: Vec<String> = cand_tokens.intersection(&ex_tokens).cloned().collect();
    if shared.len() >= 2 {
        let mut sorted = shared;
        sorted.sort();
        return Some(format!(
            "主要キーワード（{}）が既存シナリオと重複しています",
            sorted.join(", ")
        ));
    }

    None
}

fn extract_ascii_tokens(text: &str) -> HashSet<String> {
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .map(|s| s.to_ascii_lowercase())
        .filter(|s| s.len() >= 3 && !matches!(s.as_str(), "the" | "and" | "for" | "cli" | "test"))
        .collect()
}

fn next_file_prefix(dir: &Path) -> Result<usize> {
    let mut max_num = 0usize;
    if dir.exists() {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            if let Some(name) = entry.file_name().to_str()
                && let Some((prefix, _)) = name.split_once('-')
                && let Ok(num) = prefix.parse::<usize>()
            {
                max_num = max_num.max(num);
            }
        }
    }
    Ok(max_num + 1)
}
