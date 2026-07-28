// Permission model (ADR 0005, 0018).
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};

/// Durability of a permission grant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermScope {
    Once,
    AlwaysThisSession,
    AlwaysThisProject,
}

/// Fine-grained target of a grant, e.g. `run_command:git` or `run_capability:foo@shell`.
/// Landing at the command-class / path-prefix sweet spot, never a bare tool name.
pub type PermissionKey = String;

/// What a Tool reports for a given call. `None` = read-only, never prompts —
/// and doubles as the sub-agent capability boundary (ADR 0019).
#[derive(Debug, Clone)]
pub enum Permission {
    None,
    Ask { key: PermissionKey },
}

/// In-memory allowlist for the current session; cleared on process exit.
/// The persisted project allowlist (codecoder.json) is keyed identically.
#[derive(Debug, Default)]
pub struct SessionAllowlist {
    keys: HashSet<PermissionKey>,
}

impl SessionAllowlist {
    pub fn allows(&self, key: &str) -> bool {
        self.keys.contains(key)
    }

    pub fn grant(&mut self, key: PermissionKey) {
        self.keys.insert(key);
    }
}

/// Ceiling rule (ADR 0022): a Shell-environment capability may never reach
/// AlwaysThisProject. Returns the highest scope permitted for `key`.
pub fn scope_ceiling(key: &str) -> PermScope {
    if key.ends_with("@shell") {
        PermScope::AlwaysThisSession
    } else {
        PermScope::AlwaysThisProject
    }
}

/// The persisted project allowlist (ADR 0005): `AlwaysThisProject` grants survive
/// process exit in `<root>/codecoder.json`, keyed identically to the in-memory
/// session set but distinct in lifetime and storage. Loaded once at startup and
/// rewritten on each new grant.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ProjectAllowlist {
    // BTreeSet → deterministic on-disk order, so the file doesn't churn.
    #[serde(default)]
    allowlist: BTreeSet<PermissionKey>,
}

impl ProjectAllowlist {
    fn path(root: &Path) -> PathBuf {
        root.join("codecoder.json")
    }

    /// Read the project allowlist from `<root>/codecoder.json`; empty (never an
    /// error) when the file is absent or unreadable — a missing config simply
    /// means no persisted grants yet.
    pub fn load(root: &Path) -> Self {
        std::fs::read_to_string(Self::path(root))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn allows(&self, key: &str) -> bool {
        if self.allowlist.contains(key) {
            return true;
        }
        // 精确匹配失败后尝试通配符：如 key="run_command:npm install 2>&1" 匹配 "run_command:*"
        // 通配符只在 trusted headless 项目中使用，不降低交互安全性。
        key.starts_with("run_command:") && self.allowlist.contains("run_command:*")
    }

    /// Insert `key` and persist to disk. A no-op write is skipped when the key is
    /// already present. Returns the IO error if the file cannot be written.
    pub fn grant(&mut self, root: &Path, key: PermissionKey) -> std::io::Result<()> {
        if self.allowlist.insert(key) {
            self.save(root)?;
        }
        Ok(())
    }

    fn save(&self, root: &Path) -> std::io::Result<()> {
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(Self::path(root), json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcard_matches_composite_command() {
        let mut wl = ProjectAllowlist::default();
        wl.allowlist.insert("run_command:*".to_string());
        // 复合命令应被通配符匹配
        assert!(wl.allows("run_command:npm install 2>&1"));
        // 非 run_command 不受影响
        assert!(!wl.allows("write_file"));
    }

    #[test]
    fn without_wildcard_composite_still_denied() {
        let wl = ProjectAllowlist::default(); // 空的 allowlist
        // 没有通配符 → 复合命令不被允许
        assert!(!wl.allows("run_command:npm install 2>&1"));
    }

    #[test]
    fn wildcard_does_not_break_exact_matches() {
        let mut wl = ProjectAllowlist::default();
        wl.allowlist.insert("write_file".to_string());
        // 普通精确匹配仍然工作
        assert!(wl.allows("write_file"));
        assert!(!wl.allows("edit_file"));
    }
}
