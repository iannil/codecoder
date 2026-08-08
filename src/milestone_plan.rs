//! Milestone plan persistence (design 2026-08-08).
//! Plans live under `.codecoder/milestone-plans/N-plan.json`.
//! Each plan records the engineer skill used, acceptance criteria, scope, and risks.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MilestonePlan {
    pub milestone_id: u64,
    pub title: String,
    pub skill_used: String,
    pub created_at: String,
    pub acceptance_criteria: Vec<String>,
    pub scope: MilestoneScope,
    pub risks: Vec<String>,
    pub test_requirements: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MilestoneScope {
    pub files_to_create: Vec<String>,
    pub files_to_modify: Vec<String>,
    pub estimated_lines: u64,
}

/// Directory for milestone plans: `<root>/.codecoder/milestone-plans/`
pub fn plan_dir(root: &Path) -> PathBuf {
    root.join(".codecoder").join("milestone-plans")
}

/// Full path for milestone #N's plan: `<root>/.codecoder/milestone-plans/N-plan.json`
pub fn plan_path(root: &Path, milestone_id: u64) -> PathBuf {
    plan_dir(root).join(format!("{}-plan.json", milestone_id))
}

/// Check if a plan exists for milestone #N (without loading it).
pub fn plan_exists(root: &Path, milestone_id: u64) -> bool {
    plan_path(root, milestone_id).exists()
}

/// Write a plan to disk, creating the directory if needed.
pub fn write_plan(root: &Path, plan: &MilestonePlan) -> anyhow::Result<()> {
    let dir = plan_dir(root);
    std::fs::create_dir_all(&dir)?;
    let path = plan_path(root, plan.milestone_id);
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(plan)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Read a plan for milestone #N. Returns error if missing or corrupt.
pub fn read_plan(root: &Path, milestone_id: u64) -> anyhow::Result<MilestonePlan> {
    let path = plan_path(root, milestone_id);
    let raw = std::fs::read_to_string(&path)?;
    Ok(serde_json::from_str(&raw)?)
}

/// Read all plans for all milestones that have them.
pub fn all_plans(root: &Path) -> Vec<MilestonePlan> {
    let dir = plan_dir(root);
    let Ok(entries) = std::fs::read_dir(&dir) else { return vec![] };
    let mut plans = Vec::new();
    for entry in entries {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        if let Ok(raw) = std::fs::read_to_string(&path) {
            if let Ok(plan) = serde_json::from_str::<MilestonePlan>(&raw) {
                plans.push(plan);
            }
        }
    }
    plans
}

/// Delete a plan for milestone #N. No-op if missing.
pub fn delete_plan(root: &Path, milestone_id: u64) {
    let path = plan_path(root, milestone_id);
    let _ = std::fs::remove_file(&path);
}