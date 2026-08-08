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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn plan_path_format() {
        let root = Path::new("/tmp/project");
        let p = plan_path(root, 42);
        assert!(p.to_str().unwrap().contains(".codecoder/milestone-plans/42-plan.json"));
    }

    #[test]
    fn write_and_read_plan_roundtrip() {
        let dir = std::env::temp_dir().join(format!("cc_plan_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let plan = MilestonePlan {
            milestone_id: 1,
            title: "Build data model".into(),
            skill_used: "engineer-architect".into(),
            created_at: "2026-08-08T10:00:00Z".into(),
            acceptance_criteria: vec!["Fields have types".into()],
            scope: MilestoneScope {
                files_to_create: vec!["src/model.rs".into()],
                files_to_modify: vec![],
                estimated_lines: 100,
            },
            risks: vec!["Migration risk".into()],
            test_requirements: "Unit tests for each model".into(),
        };
        write_plan(&dir, &plan).unwrap();
        assert!(plan_exists(&dir, 1));
        let back = read_plan(&dir, 1).unwrap();
        assert_eq!(back.milestone_id, 1);
        assert_eq!(back.title, "Build data model");
        assert_eq!(back.skill_used, "engineer-architect");
        // Cleanup
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn plan_exists_returns_false_for_missing() {
        let dir = std::env::temp_dir().join(format!("cc_plan_miss_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!plan_exists(&dir, 99));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn all_plans_returns_empty_for_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        assert!(all_plans(dir.path()).is_empty());
    }

    #[test]
    fn delete_plan_removes_file() {
        let dir = std::env::temp_dir().join(format!("cc_plan_del_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let plan = MilestonePlan {
            milestone_id: 1,
            title: "Test".into(),
            skill_used: "engineer-coach".into(),
            created_at: "2026-08-08T00:00:00Z".into(),
            acceptance_criteria: vec![],
            scope: MilestoneScope {
                files_to_create: vec![],
                files_to_modify: vec![],
                estimated_lines: 0,
            },
            risks: vec![],
            test_requirements: String::new(),
        };
        write_plan(&dir, &plan).unwrap();
        assert!(plan_exists(&dir, 1));
        delete_plan(&dir, 1);
        assert!(!plan_exists(&dir, 1));
        std::fs::remove_dir_all(&dir).ok();
    }
}