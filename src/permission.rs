// Permission model (ADR 0005, 0018).
use std::collections::HashSet;

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
