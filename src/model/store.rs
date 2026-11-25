use std::collections::{HashMap, HashSet};
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

#[derive(Debug, Clone)]
pub struct TreeEntry {
    pub loaded: LoadedScenario,
    /// Nesting depth (`0` for root scenarios, `1+` for attached Togaki).
    pub depth: usize,
    /// Whether this entry is the last sibling under its parent (for `└─` vs `├─` rendering).
    pub is_last_sibling: bool,
    /// If an ancestor scenario has not passed yet, contains that ancestor's ID.
    pub blocked_by_parent: Option<String>,
}

/// Loads all valid `.md` scenario/togaki files from `dir`, sorted by `(priority, filename)`.
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

/// Orders scenarios and child Togaki into a parent-first depth-first tree,
/// computing parent-blocking status for each Togaki.
pub fn order_as_tree(loaded: Vec<LoadedScenario>) -> Vec<TreeEntry> {
    let by_id: HashMap<String, usize> = loaded
        .iter()
        .enumerate()
        .map(|(i, s)| (s.scenario.front_matter.id.clone(), i))
        .collect();

    let mut children_map: HashMap<String, Vec<usize>> = HashMap::new();
    let mut roots: Vec<usize> = Vec::new();

    for (idx, item) in loaded.iter().enumerate() {
        if let Some(ref parent_id) = item.scenario.front_matter.parent
            && by_id.contains_key(parent_id)
        {
            children_map.entry(parent_id.clone()).or_default().push(idx);
        } else {
            roots.push(idx);
        }
    }

    struct WalkState<'a> {
        loaded: &'a [LoadedScenario],
        children_map: &'a HashMap<String, Vec<usize>>,
        visited: HashSet<usize>,
        out: Vec<TreeEntry>,
    }

    impl WalkState<'_> {
        fn walk(
            &mut self,
            idx: usize,
            depth: usize,
            is_last_sibling: bool,
            blocking_ancestor: Option<String>,
        ) {
            if !self.visited.insert(idx) {
                return;
            }
            let item = &self.loaded[idx];
            let id = item.scenario.front_matter.id.clone();

            self.out.push(TreeEntry {
                loaded: item.clone(),
                depth,
                is_last_sibling,
                blocked_by_parent: blocking_ancestor.clone(),
            });

            let next_blocker = blocking_ancestor.or_else(|| {
                if item.scenario.unlocks_children() {
                    None
                } else {
                    Some(id.clone())
                }
            });

            if let Some(child_indices) = self.children_map.get(&id) {
                let total = child_indices.len();
                for (pos, &child_idx) in child_indices.iter().enumerate() {
                    self.walk(
                        child_idx,
                        depth + 1,
                        pos + 1 == total,
                        next_blocker.clone(),
                    );
                }
            }
        }
    }

    let mut state = WalkState {
        loaded: &loaded,
        children_map: &children_map,
        visited: HashSet::new(),
        out: Vec::with_capacity(loaded.len()),
    };

    let root_total = roots.len();
    for (pos, root_idx) in roots.into_iter().enumerate() {
        state.walk(root_idx, 0, pos + 1 == root_total, None);
    }

    // Fallback in case of any unvisited cycle entries.
    for (idx, item) in loaded.iter().enumerate() {
        if state.visited.insert(idx) {
            state.out.push(TreeEntry {
                loaded: item.clone(),
                depth: 0,
                is_last_sibling: true,
                blocked_by_parent: None,
            });
        }
    }

    state.out
}

pub fn load_scenarios_tree(dir: &Path) -> Result<Vec<TreeEntry>> {
    let loaded = load_scenarios_from_dir(dir)?;
    Ok(order_as_tree(loaded))
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
