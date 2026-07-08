// Session persistence + migration (ADR 0004). Full fidelity on disk; autosaved
// on every append; loaded via a versioned forward-migration chain.
use crate::message::{Message, MessageId};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const SCHEMA_VERSION: u32 = 1;

pub fn sessions_dir(root: &Path) -> PathBuf {
    root.join("sessions")
}

/// The most recently modified session file under `root/sessions/`, if any.
pub fn latest_session(root: &Path) -> Option<PathBuf> {
    let dir = sessions_dir(root);
    let mut newest: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let modified = entry.metadata().and_then(|m| m.modified()).ok()?;
        if newest.as_ref().map(|(t, _)| modified > *t).unwrap_or(true) {
            newest = Some((modified, path));
        }
    }
    newest.map(|(_, p)| p)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub schema_version: u32,
    pub model: String,
    pub token_count: u64,
    pub messages: Vec<Message>,
}

impl Session {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            model: model.into(),
            token_count: 0,
            messages: Vec::new(),
        }
    }

    /// Highest existing MessageId + 1 (0 when empty). Used to continue numbering
    /// after a resume without colliding with persisted ids.
    pub fn next_message_id(&self) -> MessageId {
        self.messages.iter().map(|m| m.id).max().map(|m| m + 1).unwrap_or(0)
    }

    /// Atomically persist to `path` (write temp + rename) so a crash mid-write
    /// never corrupts the session file (ADR 0004).
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self)?)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Load raw JSON and migrate forward to the current SCHEMA_VERSION.
    /// A migration failure errors and leaves the original file untouched.
    pub fn load(raw: &str) -> anyhow::Result<Session> {
        let mut json: serde_json::Value = serde_json::from_str(raw)?;
        let mut version = json
            .get("schema_version")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32;
        if version > SCHEMA_VERSION {
            anyhow::bail!(
                "session schema_version {version} is newer than supported {SCHEMA_VERSION}; refusing to mis-read"
            );
        }
        while version < SCHEMA_VERSION {
            json = migrate(version, json)?;
            version += 1;
        }
        Ok(serde_json::from_value(json)?)
    }
}

/// Forward-migration chain: migrate a session JSON from `from` to `from + 1`.
fn migrate(from: u32, json: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    match from {
        // 0 -> 1: the initial schema; nothing to transform yet.
        0 => Ok(json),
        other => anyhow::bail!("no migration registered from schema_version {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{Message, Role};

    #[test]
    fn save_load_roundtrip_and_next_id() {
        let dir = std::env::temp_dir().join(format!("cc_sess_{}", std::process::id()));
        let path = sessions_dir(&dir).join("s.json");

        let mut s = Session::new("gpt-4o");
        s.messages.push(Message::text(0, Role::User, "hi"));
        s.messages.push(Message::text(1, Role::Assistant, "yo"));
        s.save(&path).unwrap();

        let raw = std::fs::read_to_string(&path).unwrap();
        let loaded = Session::load(&raw).unwrap();
        assert_eq!(loaded.messages.len(), 2);
        assert_eq!(loaded.next_message_id(), 2);
        assert_eq!(latest_session(&dir).unwrap(), path);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_rejects_unknown_future_version() {
        let raw = r#"{"schema_version": 999, "model": "m", "token_count": 0, "messages": []}"#;
        assert!(Session::load(raw).is_err());
    }
}
