// Memory (see CONTEXT.md): a persistent key-value store under memory/, surviving
// across sessions. "Filesystem as self" — each key is a file `memory/<key>`, so
// memories are individually inspectable/editable. Doubles as the discoverable
// index of locally-stored data (`data:<name> -> {path, source, fetched, desc}`).
//
// **Cross-session by design**: Memory is shared across all sessions within a project
// because it's a file-level KV store under the project root. Any session can read
// or write any memory key, enabling long-term knowledge retention across conversations.
use std::path::{Path, PathBuf};

pub fn dir(root: &Path) -> PathBuf {
    root.join("memory")
}

/// A key is a filename directly under `memory/`; reject path separators / traversal.
pub fn key_ok(key: &str) -> bool {
    !key.is_empty() && !key.contains('/') && !key.contains("..")
}

fn key_path(root: &Path, key: &str) -> PathBuf {
    dir(root).join(key)
}

pub fn get(root: &Path, key: &str) -> Option<String> {
    if !key_ok(key) {
        return None;
    }
    std::fs::read_to_string(key_path(root, key)).ok()
}

pub fn set(root: &Path, key: &str, value: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(dir(root))?;
    std::fs::write(key_path(root, key), value)
}

pub fn remove(root: &Path, key: &str) -> std::io::Result<()> {
    std::fs::remove_file(key_path(root, key))
}

/// All memory keys (filenames under memory/), sorted.
pub fn list(root: &Path) -> Vec<String> {
    let mut keys: Vec<String> = std::fs::read_dir(dir(root))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    keys.sort();
    keys
}
