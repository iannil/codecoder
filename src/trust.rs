// Project trust (ADR 0028): a load-time gate, orthogonal to the runtime
// permission gate (ADR 0005/0018). "Filesystem as self" means a cloned repo's
// AGENTS.md/skills/prompts/capabilities — and its codecoder.json execution
// allowlist — would silently become part of the agent's identity. Trust decides
// whether that disk "self" may load at all.
//
// Decisions are stored GLOBALLY (never in the project — a repo must not vouch
// for itself), keyed by canonical directory, resolved nearest-ancestor.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustDecision {
    Trusted,
    Untrusted,
}

/// The on-disk store: canonical dir → decision. BTreeMap → deterministic order.
#[derive(Debug, Default, Serialize, Deserialize)]
struct TrustStore {
    #[serde(default)]
    decisions: BTreeMap<String, String>,
}

/// The global trust file: `CODECODER_TRUST_FILE` override, else `~/.codecoder/trust.json`.
pub fn store_path() -> PathBuf {
    if let Ok(p) = std::env::var("CODECODER_TRUST_FILE") {
        return PathBuf::from(p);
    }
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join(".codecoder").join("trust.json")
}

/// Canonicalize for a stable key; fall back to the raw path when the dir doesn't
/// exist yet (canonicalize would error).
fn canon(root: &Path) -> String {
    root.canonicalize()
        .unwrap_or_else(|_| root.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

fn load(store: &Path) -> TrustStore {
    std::fs::read_to_string(store)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn parse(v: &str) -> TrustDecision {
    match v {
        "trusted" => TrustDecision::Trusted,
        _ => TrustDecision::Untrusted,
    }
}

/// Nearest-ancestor lookup against an explicit store (hermetic — used by tests).
/// `root` itself is checked first, then each parent; a trusted parent trusts its
/// children.
pub fn decide_in(store: &Path, root: &Path) -> Option<TrustDecision> {
    let map = load(store);
    if map.decisions.is_empty() {
        return None;
    }
    let start = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    for anc in start.ancestors() {
        if let Some(v) = map.decisions.get(&anc.to_string_lossy().into_owned()) {
            return Some(parse(v));
        }
    }
    None
}

/// Persist a decision for `root` into an explicit store (hermetic — used by tests).
pub fn record_in(store: &Path, root: &Path, decision: TrustDecision) {
    let mut s = load(store);
    let value = match decision {
        TrustDecision::Trusted => "trusted",
        TrustDecision::Untrusted => "untrusted",
    };
    s.decisions.insert(canon(root), value.to_string());
    if let Some(parent) = store.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(&s) {
        let _ = std::fs::write(store, json);
    }
}

/// The persisted decision for `root`, or `None` if undecided.
pub fn decide(root: &Path) -> Option<TrustDecision> {
    decide_in(&store_path(), root)
}

/// Persist a decision for `root` in the global store.
pub fn record(root: &Path, decision: TrustDecision) {
    record_in(&store_path(), root, decision)
}

/// Whether `root` contains any trust-requiring "self" on disk (ADR 0028). When it
/// has none, there is nothing to gate — an undecided project needs no prompt.
/// Mirrors pi's TRUST_REQUIRING_PROJECT_CONFIG_RESOURCES.
pub fn has_config_resources(root: &Path) -> bool {
    const FILES: [&str; 3] = ["AGENTS.md", "CONTEXT.md", "codecoder.json"];
    const DIRS: [&str; 3] = ["skills", "prompts", "capabilities"];
    FILES.iter().any(|f| root.join(f).is_file())
        || DIRS.iter().any(|d| root.join(d).is_dir())
}

/// The fallback when no user can be prompted (headless), from
/// `CODECODER_DEFAULT_TRUST`. `never` (or unset) → Untrusted; `always`/`once` →
/// Trusted. Never persisted — a default is per-run, not a recorded decision.
pub fn default_trust() -> TrustDecision {
    default_trust_from(std::env::var("CODECODER_DEFAULT_TRUST").ok().as_deref())
}

fn default_trust_from(v: Option<&str>) -> TrustDecision {
    match v {
        Some("always") | Some("once") => TrustDecision::Trusted,
        _ => TrustDecision::Untrusted,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("cc_trust_{}_{}", tag, std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn record_then_decide_roundtrip() {
        let base = tmp("rt");
        let store = base.join("trust.json");
        let proj = base.join("proj");
        std::fs::create_dir_all(&proj).unwrap();

        assert_eq!(decide_in(&store, &proj), None);
        record_in(&store, &proj, TrustDecision::Trusted);
        assert_eq!(decide_in(&store, &proj), Some(TrustDecision::Trusted));

        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn nearest_ancestor_inherits_and_child_overrides() {
        let base = tmp("anc");
        let store = base.join("trust.json");
        let parent = base.join("parent");
        let child = parent.join("child");
        std::fs::create_dir_all(&child).unwrap();

        // A trusted parent trusts its children.
        record_in(&store, &parent, TrustDecision::Trusted);
        assert_eq!(decide_in(&store, &child), Some(TrustDecision::Trusted));

        // An explicit child decision (nearer) overrides the ancestor.
        record_in(&store, &child, TrustDecision::Untrusted);
        assert_eq!(decide_in(&store, &child), Some(TrustDecision::Untrusted));
        // ...without changing the parent's own decision.
        assert_eq!(decide_in(&store, &parent), Some(TrustDecision::Trusted));

        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn default_trust_policy() {
        assert_eq!(default_trust_from(None), TrustDecision::Untrusted);
        assert_eq!(default_trust_from(Some("never")), TrustDecision::Untrusted);
        assert_eq!(default_trust_from(Some("always")), TrustDecision::Trusted);
        assert_eq!(default_trust_from(Some("once")), TrustDecision::Trusted);
    }
}
