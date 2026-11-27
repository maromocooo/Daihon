use std::collections::{BTreeMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::model::scenario::{
    CheckItem, CheckState, FrontMatter, Priority, Scenario, ScenarioKind, ScenarioStatus, Step,
    StepStatus, VerifyMode,
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
            "hosts": { "type": "array", "items": { "type": "string" }, "description": "Optional SSH/remote host names involved in this scenario" },
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

    build_proposal_from_agent(
        &prompt,
        &existing,
        ScenarioKind::Scenario,
        None,
        None,
        &[],
        runner,
        workdir,
        timeout_secs,
    )
}

/// Converts a natural language branch / refinement ("Togaki") attached to an existing parent
/// scenario into a validated `kind: togaki` proposal without modifying the parent file.
pub fn propose_togaki(
    parent_id: &str,
    natural_input: &str,
    attach_step: Option<usize>,
    scenarios_dir: &Path,
    runner: &dyn AgentRunner,
    workdir: &Path,
    timeout_secs: u64,
) -> Result<RegistrationProposal> {
    if natural_input.trim().is_empty() {
        bail!("ト書きの説明が空です。追加したい条件や派生ユースケースを自然言語で入力してください");
    }

    let existing = load_scenarios_from_dir(scenarios_dir)?;
    let parent_loaded = existing
        .iter()
        .find(|s| s.scenario.front_matter.id == parent_id)
        .ok_or_else(|| {
            let available: Vec<&str> = existing
                .iter()
                .map(|s| s.scenario.front_matter.id.as_str())
                .collect();
            if available.is_empty() {
                anyhow!(
                    "親シナリオ `{parent_id}` が見つかりません（シナリオがまだ登録されていません）"
                )
            } else {
                anyhow!(
                    "親シナリオ `{parent_id}` が見つかりません（登録済み ID: {}）",
                    available.join(", ")
                )
            }
        })?;

    let parent_sc = &parent_loaded.scenario;
    if let Some(step_num) = attach_step
        && (step_num == 0 || step_num > parent_sc.steps.len())
    {
        bail!(
            "`--attach-step {step_num}` は親シナリオ `{parent_id}` のステップ範囲 (1〜{}) 外です",
            parent_sc.steps.len()
        );
    }

    let siblings: Vec<&LoadedScenario> = existing
        .iter()
        .filter(|s| s.scenario.front_matter.parent.as_deref() == Some(parent_id))
        .collect();

    let prompt = build_togaki_prompt(parent_sc, &siblings, natural_input, attach_step);

    build_proposal_from_agent(
        &prompt,
        &existing,
        ScenarioKind::Togaki,
        Some(parent_id.to_string()),
        attach_step,
        &parent_sc.front_matter.hosts,
        runner,
        workdir,
        timeout_secs,
    )
}

#[allow(clippy::too_many_arguments)]
fn build_proposal_from_agent(
    prompt: &str,
    existing: &[LoadedScenario],
    kind: ScenarioKind,
    parent: Option<String>,
    attach_step: Option<usize>,
    inherited_hosts: &[String],
    runner: &dyn AgentRunner,
    workdir: &Path,
    timeout_secs: u64,
) -> Result<RegistrationProposal> {
    let response = runner.run(&AgentRequest {
        working_dir: workdir.to_path_buf(),
        prompt: prompt.to_string(),
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

    let mut hosts: Vec<String> = inherited_hosts.to_vec();
    for h in draft
        .hosts
        .into_iter()
        .map(|h| h.trim().to_string())
        .filter(|h| !h.is_empty())
    {
        if !hosts.contains(&h) {
            hosts.push(h);
        }
    }

    let scenario = Scenario {
        front_matter: FrontMatter {
            id: unique_id,
            title: draft.title.trim().to_string(),
            kind,
            parent: parent.clone(),
            attach_step,
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
        if parent.as_deref() == Some(item.existing_id.as_str()) {
            // Do not flag the intended parent scenario itself as an accidental duplicate.
            continue;
        }
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
    for ex in existing {
        let ex_id = &ex.scenario.front_matter.id;
        if parent.as_deref() == Some(ex_id.as_str()) || reported_ids.contains(ex_id) {
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
        r#"あなたはシナリオ駆動の開発ハーネス「Daihon（台本）」の登録役です。
ユーザーの自然言語による要望を、開発エージェントと検証エージェント（見張り役）が共有できる
明確なシナリオ（台本）へ分解し、指定された JSON Schema に厳密に従って返してください。

ルール:
1. id は小文字英数字とハイフンの kebab-case（例: seguito-ubuntu-blender）にする。
2. overview には、ユーザーが本当にドッグフーディングしたい使い方・最終体験を自然な日本語で残す。
3. 粒度の扱い:
   - 「MacBook Air でアプリを開き、Ubuntu 側の Blender を起動して編集できる」のような大きな E2E 体験も、小さな部品へ矮小化せず **3〜6 個の大きな到達マイルストーン（起動/接続 → リモートアプリ起動 → 編集/保存 等）** としてそのまま設計する。
   - 細かい遅延条件や派生機能はあとから `daihon togaki`（ト書き）で枝葉として追加できる前提にし、まずは本筋の E2E が通るステップに絞る。
4. チェック項目はコマンド・テスト・SSH・プロセス・API・数値計測で自動検証できるものを `auto`、実機 GUI の描画品質や操作感など人の確認が必要なものを `manual` に分類する。
   - **デスクトップ／GUI アプリ（macOS `.app`、Tauri、Electron 等）の注意**: サンドボックス内の見張り役は OS 制約によりネイティブ GUI ウィンドウを直接起動できない場合があるため、`auto` はビルド・ヘッドレス起動モード・自動テスト（単体/結合/UIテスト）で完結する条件にし、実機でのウィンドウ起動・見た目・操作感の確認は `manual` に分離すること。
5. チェック ID は `s1-c1`, `s1-c2`, `s2-c1` のように一意にする。複数マシンが登場する場合は `hosts` にも候補ホスト名を入れる。
6. 既存シナリオと重なる部分があれば `overlaps` に該当 `existing_id` と重なり理由を挙げる。
7. 既存シナリオとの相対的な重要度から `priority`（P0〜P3）と `priority_rationale` を提案する。

## 既存シナリオ一覧
{existing_summary}

## ユーザーの新しいシナリオ要望
{natural_input}
"#
    )
}

fn build_togaki_prompt(
    parent: &Scenario,
    siblings: &[&LoadedScenario],
    natural_input: &str,
    attach_step: Option<usize>,
) -> String {
    let parent_steps = parent
        .steps
        .iter()
        .map(|s| format!("  - Step {}: {}", s.number, s.title))
        .collect::<Vec<_>>()
        .join("\n");

    let sibling_summary = if siblings.is_empty() {
        "（この親シナリオ配下の既存ト書きはまだありません）".to_string()
    } else {
        siblings
            .iter()
            .map(|s| {
                let sc = &s.scenario;
                format!(
                    "- id: {} | priority: {} | title: {}",
                    sc.front_matter.id, sc.front_matter.priority, sc.front_matter.title
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    let attach_note = match attach_step {
        Some(n) => format!("\n- 特に紐づく親ステップ: Step {n}"),
        None => String::new(),
    };

    format!(
        r#"あなたはシナリオ駆動の開発ハーネス「Daihon（台本）」の **ト書き（Togaki）** 登録役です。
既に存在する親シナリオ（本筋の台本）が通った前提のうえで、ユーザーから追加された
枝葉の要件・非機能条件（例: 遅延が xxx ms 以内）・派生ユースケース（例: ローカル素材を引き渡せる）を
コンパクトなト書きとして構造化し、指定された JSON Schema に厳密に従って返してください。

ルール:
1. 親シナリオの基本ステップ（アプリ起動や基本接続など）は「既に通過済み」の前提として重複定義せず、**今回追加された差分条件・派生フローだけ** をステップ化する。
   - 単一の品質・パフォーマンス条件（例: 遅延 50ms 以内）なら 1 ステップ・1〜2 チェックで簡潔に作る。
   - 複数段階の派生機能（例: ローカル素材の選択 → 転送 → リモートアプリで読込）なら 2〜3 ステップに分ける。
2. id は親シナリオとの関係が分かる kebab-case にする（例: {parent_id}-latency）。
3. 自動計測・コマンド・SSH・自動テストで検証できる項目は `auto`、実機 GUI の見た目や人の体感確認が必要な項目は `manual` に分類する（デスクトップ GUI アプリではビルド・ヘッドレス実行・UIテストを `auto`、実画面の操作感や描画確認を `manual` に分ける）。
4. 同じ親にぶら下がる既存ト書きと重なる部分があれば `overlaps` に挙げる（親シナリオ自身は `overlaps` に入れない）。
5. 親シナリオ（{parent_priority}）との関係を踏まえ、`priority`（P0〜P3）と `priority_rationale` を提案する。

## 親シナリオ（前提となる本筋）
- ID: {parent_id}
- タイトル: {parent_title}
- 優先度: {parent_priority}
- 対象ホスト: [{parent_hosts}]{attach_note}
- 概要:
{parent_overview}
- ステップ:
{parent_steps}

## 同じ親配下の既存ト書き
{sibling_summary}

## 今回追加したいト書き（枝葉の要件）
{natural_input}
"#,
        parent_id = parent.front_matter.id,
        parent_title = parent.front_matter.title,
        parent_priority = parent.front_matter.priority,
        parent_hosts = parent.front_matter.hosts.join(", "),
        parent_overview = parent.overview,
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
