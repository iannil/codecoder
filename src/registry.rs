// Registry (ADR 0020): scans skills/ and capabilities/ into a resident catalog
// (name + one-line description). Full text / execution happen only on activation.
use crate::trust::{SourceInfo, SourceOrigin, SourceScope};
use std::path::Path;
use std::sync::{Arc, RwLock};
use std::time::SystemTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Skill,
    /// Draft tier of the Skill kind (ADR 0025): a `prompts/` entry, activated the
    /// same way as a Skill via `use_skill`, but lower maturity and lower priority.
    Prompt,
    Capability,
}

#[derive(Debug, Clone)]
pub struct CatalogEntry {
    pub name: String,
    pub description: String,
    pub kind: EntryKind,
    /// Provenance of this entry (first-class citizen #5). `None` for legacy or
    /// synthetic entries that have not been migrated.
    pub source: Option<SourceInfo>,
}

/// The scanner/index (distinct from the lightweight `catalog` it produces).
#[derive(Debug, Default)]
pub struct Registry {
    pub catalog: Vec<CatalogEntry>,
}

impl Registry {
    /// Scan at startup and on `/reload`. Shell-authored changes only take effect
    /// through this reload — no hot registration (ADR 0022).
    pub fn scan(root: &Path) -> Self {
        let mut catalog = Vec::new();
        scan_skills(&root.join("skills"), SourceScope::Project, &mut catalog);
        scan_prompts(&root.join("prompts"), SourceScope::Project, &mut catalog);
        scan_capabilities(&root.join("capabilities"), SourceScope::Project, &mut catalog);
        catalog.sort_by(|a, b| a.name.cmp(&b.name));
        Registry { catalog }
    }

    /// Render the resident catalog for injection into the system prompt (ADR 0020).
    pub fn render_catalog(&self) -> String {
        if self.catalog.is_empty() {
            return String::new();
        }
        let mut skills = Vec::new();
        let mut drafts = Vec::new();
        let mut caps = Vec::new();
        for e in &self.catalog {
            let line = format!("- {} — {}", e.name, e.description);
            match e.kind {
                EntryKind::Skill => skills.push(line),
                EntryKind::Prompt => drafts.push(line),
                EntryKind::Capability => caps.push(line),
            }
        }
        let mut out = String::new();
        if !skills.is_empty() || !drafts.is_empty() {
            out.push_str("Skills (activate with the `use_skill` tool):\n");
            // Matured Skills first; lower-priority Prompt drafts after (ADR 0025).
            out.push_str(&skills.join("\n"));
            if !skills.is_empty() && !drafts.is_empty() {
                out.push('\n');
            }
            out.push_str(&drafts.join("\n"));
            out.push('\n');
        }
        if !caps.is_empty() {
            out.push_str("Capabilities (run with the `run_capability` tool):\n");
            out.push_str(&caps.join("\n"));
            out.push('\n');
        }
        out
    }

    /// 重新扫描，覆盖当前 catalog（daemon 共享实例在写盘后调用）。
    pub fn reload(&mut self, root: &Path) {
        let fresh = Registry::scan(root);
        self.catalog = fresh.catalog;
    }
}

/// Remembered mtimes of the three scanned directories, kept between reload
/// ticks. `Default` is all-`None` (first tick records without reporting change).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DirMtimes {
    pub skills: Option<SystemTime>,
    pub capabilities: Option<SystemTime>,
    pub prompts: Option<SystemTime>,
}

/// Stat `skills/`, `capabilities/`, `prompts/` under `root` and compare to
/// `last`. Returns `true` iff any observed mtime differs from the previously
/// remembered value; updates `last` to the just-observed mtimes. The FIRST
/// observation of a dir (slot was `None`) records the mtime but does NOT count
/// as a change (the startup scan already populated the Registry). A failed
/// stat (missing dir) leaves the prior value untouched — the catalog is never
/// clobbered by a transient stat error.
pub fn mtime_changed(root: &Path, last: &mut DirMtimes) -> bool {
    let slots: [(&str, &mut Option<SystemTime>); 3] = [
        ("skills", &mut last.skills),
        ("capabilities", &mut last.capabilities),
        ("prompts", &mut last.prompts),
    ];
    let mut changed = false;
    for (sub, slot) in slots {
        match root.join(sub).metadata().and_then(|m| m.modified()) {
            Ok(mtime) => match *slot {
                None => *slot = Some(mtime),                 // first observation: record, no change
                Some(prev) if prev != mtime => { *slot = Some(mtime); changed = true; }
                _ => {}                                       // unchanged
            },
            Err(_) => {}                                       // keep prior; no change
        }
    }
    changed
}

/// If any scanned dir's mtime changed since `last`, re-scan into the shared
/// Registry under a write lock. Otherwise no-op. Call this on a timer (the
/// daemon reload thread) — it has no internal clock, so it is directly testable.
pub fn tick_reload(reg: &Arc<RwLock<Registry>>, root: &Path, last: &mut DirMtimes) {
    if mtime_changed(root, last) {
        reg.write().unwrap().reload(root);
    }
}

fn scan_skills(dir: &Path, scope: SourceScope, out: &mut Vec<CatalogEntry>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let path = e.path();
        if path.extension().and_then(|x| x.to_str()) != Some("md") {
            continue;
        }
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let (name, description) = parse_skill_meta(&text, &stem);
        let source = Some(SourceInfo {
            path: crate::trust::canon_path(&path),
            scope,
            origin: SourceOrigin::TopLevel,
        });
        out.push(CatalogEntry { name, description, kind: EntryKind::Skill, source });
    }
}

/// Scan `prompts/` (ADR 0025): draft-tier Skills. Same `.md` shape as a Skill, but
/// the description is prefixed `[draft]` so the agent prefers matured Skills.
fn scan_prompts(dir: &Path, scope: SourceScope, out: &mut Vec<CatalogEntry>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let path = e.path();
        if path.extension().and_then(|x| x.to_str()) != Some("md") {
            continue;
        }
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let (name, description) = parse_skill_meta(&text, &stem);
        let source = Some(SourceInfo {
            path: crate::trust::canon_path(&path),
            scope,
            origin: SourceOrigin::TopLevel,
        });
        out.push(CatalogEntry {
            name,
            description: format!("[draft] {description}"),
            kind: EntryKind::Prompt,
            source,
        });
    }
}

fn scan_capabilities(dir: &Path, scope: SourceScope, out: &mut Vec<CatalogEntry>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let manifest = e.path().join("manifest.json");
        let Ok(raw) = std::fs::read_to_string(&manifest) else { continue };
        let Ok(m) = serde_json::from_str::<crate::capability::CapabilityManifest>(&raw) else { continue };
        let source = Some(SourceInfo {
            path: crate::trust::canon_path(&manifest),
            scope,
            origin: SourceOrigin::TopLevel,
        });
        out.push(CatalogEntry {
            name: m.name,
            description: m.description,
            kind: EntryKind::Capability,
            source,
        });
    }
}

/// Extract (name, description) from a skill `.md`: optional `--- name: / description: ---`
/// frontmatter, else name = filename stem and description = first non-empty line.
fn parse_skill_meta(text: &str, stem: &str) -> (String, String) {
    let mut name = stem.to_string();
    let mut description = String::new();

    let rest = if let Some(body) = text.strip_prefix("---") {
        if let Some(end) = body.find("\n---") {
            let front = &body[..end];
            for line in front.lines() {
                if let Some(v) = line.strip_prefix("name:") {
                    name = v.trim().to_string();
                } else if let Some(v) = line.strip_prefix("description:") {
                    description = v.trim().to_string();
                }
            }
            &body[end + 4..]
        } else {
            text
        }
    } else {
        text
    };

    if description.is_empty() {
        description = rest
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty() && !l.starts_with('#'))
            .unwrap_or("")
            .to_string();
    }
    (name, description)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, RwLock};

    #[test]
    fn scans_skill_frontmatter_and_capability_manifest() {
        let dir = std::env::temp_dir().join(format!("cc_reg_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("skills")).unwrap();
        std::fs::create_dir_all(dir.join("capabilities/fetcher")).unwrap();
        std::fs::write(
            dir.join("skills/reviewing.md"),
            "---\nname: reviewing\ndescription: how to review a PR\n---\nbody here",
        )
        .unwrap();
        std::fs::write(
            dir.join("capabilities/fetcher/manifest.json"),
            r#"{"name":"fetcher","description":"fetch a url","environment":"shell","lifecycle":"one_shot","entry":"curl $URL"}"#,
        )
        .unwrap();

        let reg = Registry::scan(&dir);
        assert_eq!(reg.catalog.len(), 2);
        let cat = reg.render_catalog();
        assert!(cat.contains("reviewing — how to review a PR"));
        assert!(cat.contains("fetcher — fetch a url"));

        // Each entry carries SourceInfo (first-class citizen #5).
        for e in &reg.catalog {
            let src = e.source.as_ref().expect("each scanned entry must have SourceInfo");
            assert!(src.path.contains("capabilities/fetcher/manifest.json") || src.path.contains("skills/reviewing.md"));
            assert_eq!(src.scope, crate::trust::SourceScope::Project);
            assert_eq!(src.origin, crate::trust::SourceOrigin::TopLevel);
        }

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn skill_meta_falls_back_to_stem_and_first_line() {
        let (n, d) = parse_skill_meta("# Title\n\nDo the thing.\n", "myskill");
        assert_eq!(n, "myskill");
        assert_eq!(d, "Do the thing.");
    }

    #[test]
    fn reload_picks_up_newly_written_skill() {
        let dir = std::env::temp_dir().join(format!("cc_regreload_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("skills")).unwrap();
        let mut reg = Registry::scan(&dir);
        assert!(reg.catalog.is_empty());
        std::fs::write(dir.join("skills/new.md"), "---\nname: new\ndescription: d\n---\nb").unwrap();
        reg.reload(&dir);
        assert_eq!(reg.catalog.len(), 1);
        assert_eq!(reg.catalog[0].name, "new");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mtime_changed_first_call_records_and_returns_false() {
        let dir = std::env::temp_dir().join(format!("cc_mtime_first_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("skills")).unwrap();
        let mut last = DirMtimes::default();
        assert!(!mtime_changed(&dir, &mut last), "first call records, no change");
        assert!(last.skills.is_some(), "skills mtime recorded");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tick_reload_picks_up_new_skill() {
        let dir = std::env::temp_dir().join(format!("cc_reload_new_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("skills")).unwrap();
        let reg = Arc::new(RwLock::new(Registry::default()));
        let mut last = DirMtimes::default();
        tick_reload(&reg, &dir, &mut last); // records mtimes, catalog empty
        assert!(reg.read().unwrap().catalog.is_empty());
        std::fs::write(
            dir.join("skills/x.md"),
            "---\nname: x\ndescription: d\n---\nbody",
        ).unwrap();
        tick_reload(&reg, &dir, &mut last); // detects change, rescans
        let cat = &reg.read().unwrap().catalog;
        assert_eq!(cat.len(), 1);
        assert_eq!(cat[0].name, "x");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tick_reload_noop_when_unchanged() {
        let dir = std::env::temp_dir().join(format!("cc_reload_noop_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("skills")).unwrap();
        std::fs::write(dir.join("skills/x.md"), "---\nname: x\ndescription: d\n---\nb").unwrap();
        let reg = Arc::new(RwLock::new(Registry::default()));
        let mut last = DirMtimes::default();
        tick_reload(&reg, &dir, &mut last); // loads x
        let n = reg.read().unwrap().catalog.len();
        tick_reload(&reg, &dir, &mut last); // no change
        assert_eq!(reg.read().unwrap().catalog.len(), n);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tick_reload_skips_missing_dirs() {
        let dir = std::env::temp_dir().join(format!("cc_reload_missing_{}", std::process::id()));
        // intentionally do NOT create skills/capabilities/prompts
        let reg = Arc::new(RwLock::new(Registry::default()));
        let mut last = DirMtimes::default();
        tick_reload(&reg, &dir, &mut last); // must not panic, must not clobber
        assert!(reg.read().unwrap().catalog.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
