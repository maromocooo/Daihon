use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::model::parser::update_scenario_markdown;
use crate::model::scenario::Scenario;
use crate::model::validator::{validate_file, validate_str};

#[derive(Debug, Clone)]
pub struct LoadedScenario {
    pub path: PathBuf,
    pub scenario: Scenario,
}

/// Loads all valid `.md` scenario files from `dir`, sorted by `(priority, filename)`.
pub fn load_scenarios_from_dir(dir: &Path) -> Result<Vec<LoadedScenario>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    if !dir.is_dir() {
        bail!("シナリオディレクトリではありません: {}", dir.display());
    }

    let mut md_paths: Vec<PathBuf> = fs::read_dir(dir)
        .with_context(|| format!("ディレクトリを読み込めません: {}", dir.display()))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().and_then(|e| e.to_str()) == Some("md"))
        .collect();
    md_paths.sort();

    let mut loaded = Vec::with_capacity(md_paths.len());
    for path in md_paths {
        match validate_file(&path) {
            Ok(scenario) => loaded.push(LoadedScenario { path, scenario }),
            Err(issues) => {
                let details = issues
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("\n  ");
                bail!("シナリオ形式エラー ({}):\n  {details}", path.display());
            }
        }
    }

    loaded.sort_by(|a, b| {
        a.scenario
            .front_matter
            .priority
            .cmp(&b.scenario.front_matter.priority)
            .then_with(|| a.path.cmp(&b.path))
    });

    Ok(loaded)
}

/// Writes an updated scenario back to disk surgically, re-validating before overwriting.
pub fn save_updated_scenario(path: &Path, scenario: &Scenario) -> Result<()> {
    let rendered = update_scenario_markdown(scenario);
    if let Err(issues) = validate_str(&rendered, Some(path)) {
        let details = issues
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n  ");
        bail!("書き戻し前の整合性チェックに失敗しました:\n  {details}");
    }
    fs::write(path, rendered)
        .with_context(|| format!("シナリオファイルを書き込めません: {}", path.display()))?;
    Ok(())
}
