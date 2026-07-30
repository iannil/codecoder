// Permission model (ADR 0005, 0018).
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};


/// A single entry in the allowlist. Supports plain string keys (backward compatible)
/// and scoped entries with path constraints.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AllowlistEntry {
    /// Plain key: "run_command:npm"
    Plain(String),
    /// Scoped entry with optional constraints:
    /// {"prefix": "run_command:rm", "scope": {"project_bound": true}}
    Scoped {
        prefix: String,
        #[serde(default)]
        scope: ScopeConstraint,
    },
}

/// Constraints on an allowlist entry's usage scope.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScopeConstraint {
    /// When true, the tool call is only allowed when its cwd is within the project root.
    #[serde(default)]
    pub project_bound: bool,
}

impl ScopeConstraint {
    /// Check whether a tool call's args satisfy this constraint.
    /// `root` is the project root directory. Returns true if the constraint passes.
    pub fn check(&self, args: &serde_json::Value, root: &Path) -> bool {
        if !self.project_bound {
            return true;
        }
        match args.get("cwd").and_then(serde_json::Value::as_str) {
            None => true, // no cwd specified → defaults to project root
            Some(cwd) => {
                let cwd_path = Path::new(cwd);
                cwd_path.is_absolute() && cwd_path.starts_with(root)
            }
        }
    }
}

impl Ord for AllowlistEntry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        serde_json::to_string(self).unwrap_or_default()
            .cmp(&serde_json::to_string(other).unwrap_or_default())
    }
}

impl PartialOrd for AllowlistEntry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for AllowlistEntry {
    fn eq(&self, other: &Self) -> bool {
        serde_json::to_string(self).ok() == serde_json::to_string(other).ok()
    }
}

impl Eq for AllowlistEntry {}

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
    allowlist: BTreeSet<AllowlistEntry>,
}

impl ProjectAllowlist {
    fn path(root: &Path) -> PathBuf {
        root.join("codecoder.json")
    }

    /// Read the project allowlist from `<root>/codecoder.json`; empty (never an
    /// error) when the file is absent or unreadable — a missing config simply
    /// means no persisted grants yet.
    ///
    /// 支持两种格式：
    /// - 标准数组格式：`{"allowlist": ["write_file", "run_command:npm"]}`
    /// - 兼容 map 格式：`{"allowlist": {"write_file": "AlwaysThisProject", "run_command:npm": "AlwaysThisProject"}}`
    pub fn load(root: &Path) -> Self {
        let content = match std::fs::read_to_string(Self::path(root)) {
            Ok(c) => c,
            Err(_) => return Self::default(),
        };
        // 标准格式: {"allowlist": ["write_file", "run_command:npm"]}
        if let Ok(wl) = serde_json::from_str::<ProjectAllowlist>(&content) {
            return wl;
        }
        // 兼容格式: {"allowlist": {"write_file": "AlwaysThisProject", "run_command:npm": "AlwaysThisProject"}}
        // 顶层是 {"allowlist": {...}} 的 map，其中 allowlist 的值是 key→scope 的 map
        if let Ok(top) = serde_json::from_str::<std::collections::BTreeMap<String, serde_json::Value>>(&content) {
            if let Some(entries) = top.get("allowlist").and_then(|v| v.as_object()) {
                let keys: BTreeSet<AllowlistEntry> = entries.keys()
                    .map(|k| AllowlistEntry::Plain(k.clone()))
                    .collect();
                return ProjectAllowlist { allowlist: keys };
            }
        }
        Self::default()
    }

    /// Check if a permission key is allowed. For Scoped entries, also checks
    /// path constraints against the tool call's args and project root.
    pub fn allows(&self, key: &str, args: &serde_json::Value, root: &Path) -> bool {
        if self.allowlist.iter().any(|entry| match entry {
            AllowlistEntry::Plain(k) => {
                k == key // 精确匹配优先
            }
            AllowlistEntry::Scoped { prefix, scope } => {
                // Note: scoped prefix matching must be exact (==) not starts_with
                // to avoid false positives like "run_command:rm" matching "run_command:rmdir".
                // This was changed from starts_with to == in b870ff2, then accidentally
                // reverted to starts_with by the merge 5df6535. Using == for correctness.
                prefix == key && scope.check(args, root)
            }
        }) {
            return true;
        }
        // 精确匹配失败后尝试通配符：如 key="run_command:npm install 2>&1" 匹配 "run_command:*"
        // 通配符只在 trusted headless 项目中使用，不降低交互安全性。
        key.starts_with("run_command:") && self.allowlist.iter().any(|entry| {
            matches!(entry, AllowlistEntry::Plain(k) if k == "run_command:*")
        })
    }

    /// Insert `entry` and persist to disk. A no-op write is skipped when the entry is
    /// already present. Returns the IO error if the file cannot be written.
    pub fn grant(&mut self, root: &Path, entry: AllowlistEntry) -> std::io::Result<()> {
        if self.allowlist.insert(entry) {
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
fn wildcard_entry_deserializes() {
        let entry: AllowlistEntry = serde_json::from_str(r#""run_command:git""#).unwrap();
        assert!(matches!(entry, AllowlistEntry::Plain(k) if k == "run_command:git"));
    }

    #[test]
    fn scoped_entry_deserializes() {
        let entry: AllowlistEntry =
            serde_json::from_str(r#"{"prefix":"run_command:rm","scope":{"project_bound":true}}"#)
                .unwrap();
        assert!(matches!(entry, AllowlistEntry::Scoped { .. }));
        if let AllowlistEntry::Scoped { prefix, scope } = &entry {
            assert_eq!(prefix, "run_command:rm");
            assert!(scope.project_bound);
        }
    }

    #[test]
    fn scoped_entry_default_scope() {
        // scope omitted → default ScopeConstraint (project_bound: false)
        let entry: AllowlistEntry =
            serde_json::from_str(r#"{"prefix":"run_command:echo"}"#).unwrap();
        if let AllowlistEntry::Scoped { prefix, scope } = &entry {
            assert_eq!(prefix, "run_command:echo");
            assert!(!scope.project_bound);
        } else {
            panic!("expected Scoped variant");
        }
    }

    #[test]
    fn exact_match_no_prefix_false_positive() {
        // Prefix "run_command:rm" must NOT match "run_command:rmdir".
        let mut allowlist = ProjectAllowlist::default();
        allowlist
            .allowlist
            .insert(AllowlistEntry::Scoped { prefix: "run_command:rm".into(), scope: ScopeConstraint::default() });

        let root = Path::new("/tmp");
        assert!(allowlist.allows("run_command:rm", &serde_json::json!({}), root));
        assert!(!allowlist.allows("run_command:rmdir", &serde_json::json!({}), root));
    }

    #[test]
    fn scoped_project_bound_allowed() {
        let mut allowlist = ProjectAllowlist::default();
        allowlist
            .allowlist
            .insert(AllowlistEntry::Scoped {
                prefix: "run_command:rm".into(),
                scope: ScopeConstraint { project_bound: true },
            });

        let root = Path::new("/home/user/project");
        // cwd within project root → allowed
        assert!(allowlist.allows(
            "run_command:rm",
            &serde_json::json!({"cwd": "/home/user/project/src"}),
            root,
        ));
    }

    #[test]
    fn scoped_project_bound_denied() {
        let mut allowlist = ProjectAllowlist::default();
        allowlist
            .allowlist
            .insert(AllowlistEntry::Scoped {
                prefix: "run_command:rm".into(),
                scope: ScopeConstraint { project_bound: true },
            });

        let root = Path::new("/home/user/project");
        // cwd outside project root → denied
        assert!(!allowlist.allows(
            "run_command:rm",
            &serde_json::json!({"cwd": "/tmp"}),
            root,
        ));
    }

    #[test]
    fn scoped_no_project_bound_always_allowed() {
        let mut allowlist = ProjectAllowlist::default();
        allowlist
            .allowlist
            .insert(AllowlistEntry::Scoped {
                prefix: "run_command:echo".into(),
                scope: ScopeConstraint::default(), // project_bound: false
            });

        let root = Path::new("/home/user/project");
        // Any cwd works when project_bound is false
        assert!(allowlist.allows(
            "run_command:echo",
            &serde_json::json!({"cwd": "/tmp"}),
            root,
        ));
        assert!(allowlist.allows(
            "run_command:echo",
            &serde_json::json!({"cwd": "/home/user/project"}),
            root,
        ));
    }

    #[test]
    fn scoped_cwd_defaults_to_project_root() {
        let mut allowlist = ProjectAllowlist::default();
        allowlist
            .allowlist
            .insert(AllowlistEntry::Scoped {
                prefix: "run_command:make".into(),
                scope: ScopeConstraint { project_bound: true },
            });

        let root = Path::new("/home/user/project");
        // No cwd specified → defaults to project root → allowed
        assert!(allowlist.allows("run_command:make", &serde_json::json!({}), root));
    }

    #[test]
    fn wildcard_matches_composite_command() {
        let mut wl = ProjectAllowlist::default();
        wl.allowlist.insert(AllowlistEntry::Plain("run_command:*".into()));
        assert!(wl.allows("run_command:npm install 2>&1", &serde_json::json!({}), Path::new(".")));
        assert!(!wl.allows("write_file", &serde_json::json!({}), Path::new(".")));
    }

    #[test]
    fn without_wildcard_composite_still_denied() {
        let wl = ProjectAllowlist::default();
        assert!(!wl.allows("run_command:npm install 2>&1", &serde_json::json!({}), Path::new(".")));
    }

    #[test]
    fn wildcard_does_not_break_exact_matches() {
        let mut wl = ProjectAllowlist::default();
        wl.allowlist.insert(AllowlistEntry::Plain("write_file".into()));
        assert!(wl.allows("write_file", &serde_json::json!({}), Path::new(".")));
        assert!(!wl.allows("edit_file", &serde_json::json!({}), Path::new(".")));
    }

    #[test]
    fn map_format_loads_correctly() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // map 格式: {"allowlist": {"write_file": "AlwaysThisProject", "run_command:npm": "AlwaysThisProject"}}
        let map_json = r#"{"allowlist": {"write_file": "AlwaysThisProject", "run_command:npm": "AlwaysThisProject"}}"#;
        std::fs::write(root.join("codecoder.json"), map_json).unwrap();

        let wl = ProjectAllowlist::load(root);
        assert!(wl.allows("write_file", &serde_json::json!({}), root));
        assert!(wl.allows("run_command:npm", &serde_json::json!({}), root));
        assert!(!wl.allows("edit_file", &serde_json::json!({}), root));
        assert!(!wl.allows("run_command:git", &serde_json::json!({}), root));
    }

    #[test]
    fn array_format_still_works() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // 数组格式: {"allowlist": ["write_file", "run_command:npm"]}
        let array_json = r#"{"allowlist": ["write_file", "run_command:npm"]}"#;
        std::fs::write(root.join("codecoder.json"), array_json).unwrap();

        let wl = ProjectAllowlist::load(root);
        assert!(wl.allows("write_file", &serde_json::json!({}), root));
        assert!(wl.allows("run_command:npm", &serde_json::json!({}), root));
        assert!(!wl.allows("edit_file", &serde_json::json!({}), root));
    }

    #[test]
    fn empty_json_falls_back_to_empty() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // 完全不相关格式 → 空 allowlist
        std::fs::write(root.join("codecoder.json"), "{}").unwrap();
        let wl = ProjectAllowlist::load(root);
        assert!(!wl.allows("write_file", &serde_json::json!({}), root));
    }

    #[test]
    fn missing_file_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let wl = ProjectAllowlist::load(root);
        assert!(!wl.allows("anything", &serde_json::json!({}), root));
    }
}
