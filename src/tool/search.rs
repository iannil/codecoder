// Search tools: glob (file patterns) and grep (regex over file contents). Both
// Permission::None (read-only) and available to sub-agents (ADR 0019).
// tree-sitter AST queries are a future addition (would add grammar dependencies).
use super::{Tool, ToolCtx, ToolOutput};
use crate::permission::Permission;
use regex::Regex;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use tree_sitter::{Parser, Query, QueryCursor};

const MAX_FILES: usize = 5000;
const MAX_MATCHES: usize = 200;
const SKIP_DIRS: [&str; 4] = [".git", "target", "node_modules", ".codecoder"];

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    if out.len() >= MAX_FILES {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let path = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            if SKIP_DIRS.contains(&name.as_str()) || name.starts_with('.') {
                continue;
            }
            walk(&path, out);
        } else {
            out.push(path);
        }
        if out.len() >= MAX_FILES {
            return;
        }
    }
}

pub struct Glob;

impl Tool for Glob {
    fn name(&self) -> &str {
        "glob"
    }
    fn description(&self) -> &str {
        "Find files matching a glob pattern (supports ** recursion), relative to root."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "pattern": { "type": "string", "description": "e.g. src/**/*.rs" } },
            "required": ["pattern"]
        })
    }
    fn permission(&self, _args: &Value, _root: &Path) -> Permission {
        Permission::None
    }
    fn run(&self, args: Value, ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
        let pattern = args.get("pattern").and_then(Value::as_str).unwrap_or_default();
        if pattern.is_empty() {
            return Ok(ToolOutput::err("missing required arg: pattern"));
        }
        let full = ctx.root.join(pattern);
        let Some(full_str) = full.to_str() else {
            return Ok(ToolOutput::err("non-UTF8 path"));
        };
        let paths = match glob::glob(full_str) {
            Ok(p) => p,
            Err(e) => return Ok(ToolOutput::err(format!("bad pattern: {e}"))),
        };
        let mut hits = Vec::new();
        for entry in paths.flatten() {
            let rel = entry.strip_prefix(ctx.root).unwrap_or(&entry);
            hits.push(rel.to_string_lossy().into_owned());
            if hits.len() >= MAX_MATCHES {
                break;
            }
        }
        hits.sort();
        Ok(ToolOutput::ok(if hits.is_empty() { "(no matches)".into() } else { hits.join("\n") }))
    }
}

pub struct Grep;

impl Tool for Grep {
    fn name(&self) -> &str {
        "grep"
    }
    fn description(&self) -> &str {
        "Search file contents by regex; returns path:line: match, optionally under a path."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string", "description": "A regular expression (text mode)." },
                "ast_query": { "type": "string", "description": "A tree-sitter query (AST mode), e.g. `(function_item name: (identifier) @n)`." },
                "lang": { "type": "string", "description": "Language for ast_query (default: rust)." },
                "path": { "type": "string", "description": "Subdirectory to search (default root)." }
            }
        })
    }
    fn permission(&self, _args: &Value, _root: &Path) -> Permission {
        Permission::None
    }
    fn run(&self, args: Value, ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
        let base = ctx.root.join(args.get("path").and_then(Value::as_str).unwrap_or("."));

        // AST mode (tree-sitter): run a query against source files of `lang`.
        if let Some(q) = args.get("ast_query").and_then(Value::as_str) {
            let lang = args.get("lang").and_then(Value::as_str).unwrap_or("rust");
            return Ok(match ast_search(ctx.root, &base, lang, q) {
                Ok(hits) if hits.is_empty() => ToolOutput::ok("(no matches)"),
                Ok(hits) => ToolOutput::ok(hits.join("\n")),
                Err(e) => ToolOutput::err(e),
            });
        }

        let pattern = args.get("pattern").and_then(Value::as_str).unwrap_or_default();
        if pattern.is_empty() {
            return Ok(ToolOutput::err("provide `pattern` (text) or `ast_query` (AST)"));
        }
        let re = match Regex::new(pattern) {
            Ok(r) => r,
            Err(e) => return Ok(ToolOutput::err(format!("bad regex: {e}"))),
        };
        let mut files = Vec::new();
        walk(&base, &mut files);

        let mut hits = Vec::new();
        'outer: for f in &files {
            let Ok(text) = std::fs::read_to_string(f) else { continue }; // skips binary
            let rel = f.strip_prefix(ctx.root).unwrap_or(f).to_string_lossy().into_owned();
            for (i, line) in text.lines().enumerate() {
                if re.is_match(line) {
                    let trimmed = line.trim();
                    let shown = if trimmed.len() > 200 { &trimmed[..200] } else { trimmed };
                    hits.push(format!("{rel}:{}: {shown}", i + 1));
                    if hits.len() >= MAX_MATCHES {
                        break 'outer;
                    }
                }
            }
        }
        Ok(ToolOutput::ok(if hits.is_empty() { "(no matches)".into() } else { hits.join("\n") }))
    }
}

/// Run a tree-sitter query against source files of `lang` under `base`.
/// Returns `path:line: snippet` for each captured node.
fn ast_search(root: &Path, base: &Path, lang: &str, query_str: &str) -> Result<Vec<String>, String> {
    let (language, ext) = match lang {
        "rust" | "rs" => (tree_sitter_rust::language(), "rs"),
        "python" | "py" => (tree_sitter_python::language(), "py"),
        "javascript" | "js" => (tree_sitter_javascript::language(), "js"),
        "go" => (tree_sitter_go::language(), "go"),
        "c" => (tree_sitter_c::language(), "c"),
        other => return Err(format!("unsupported lang '{other}' (supported: rust, python, javascript, go, c)")),
    };
    let query = Query::new(&language, query_str).map_err(|e| format!("bad query: {e}"))?;

    let mut files = Vec::new();
    walk(base, &mut files);

    let mut hits = Vec::new();
    for f in &files {
        if f.extension().and_then(|e| e.to_str()) != Some(ext) {
            continue;
        }
        let Ok(source) = std::fs::read_to_string(f) else { continue };
        let mut parser = Parser::new();
        if parser.set_language(&language).is_err() {
            continue;
        }
        let Some(tree) = parser.parse(&source, None) else { continue };
        let rel = f.strip_prefix(root).unwrap_or(f).to_string_lossy().into_owned();

        let mut cursor = QueryCursor::new();
        let bytes = source.as_bytes();
        let mut matches = cursor.matches(&query, tree.root_node(), bytes);
        while let Some(m) = matches.next() {
            for cap in m.captures {
                let node = cap.node;
                let row = node.start_position().row + 1;
                let text = node.utf8_text(bytes).unwrap_or("");
                let snippet = text.lines().next().unwrap_or("").trim();
                hits.push(format!("{rel}:{row}: {snippet}"));
                if hits.len() >= MAX_MATCHES {
                    return Ok(hits);
                }
            }
        }
    }
    Ok(hits)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("cc_search_{}_{tag}", std::process::id()));
        std::fs::create_dir_all(d.join("src")).unwrap();
        std::fs::write(d.join("src/main.rs"), "fn main() {\n    let answer = 42;\n}\n").unwrap();
        std::fs::write(d.join("README.md"), "# hi\nanswer here\n").unwrap();
        d
    }

    #[test]
    fn glob_matches_recursively() {
        let dir = tmp("glob");
        let mut ctx = ToolCtx { root: &dir };
        let out = Glob.run(json!({ "pattern": "**/*.rs" }), &mut ctx).unwrap();
        assert!(out.content.contains("src/main.rs"), "{}", out.content);
        assert!(!out.content.contains("README.md"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn grep_finds_regex_matches_with_locations() {
        let dir = tmp("grep");
        let mut ctx = ToolCtx { root: &dir };
        let out = Grep.run(json!({ "pattern": "answer\\s*=\\s*\\d+" }), &mut ctx).unwrap();
        assert!(out.content.contains("src/main.rs:2:"), "{}", out.content);
        // The README's "answer here" doesn't match the regex.
        assert!(!out.content.contains("README.md"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn grep_ast_query_finds_functions() {
        let dir = std::env::temp_dir().join(format!("cc_ast_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("lib.rs"), "fn alpha() {}\nfn beta(x: i32) -> i32 { x }\n").unwrap();
        let mut ctx = ToolCtx { root: &dir };
        let out = Grep
            .run(json!({ "ast_query": "(function_item name: (identifier) @n)" }), &mut ctx)
            .unwrap();
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("alpha"), "{}", out.content);
        assert!(out.content.contains("beta"), "{}", out.content);
        // A bad query reports an error.
        let bad = Grep.run(json!({ "ast_query": "(nonsense" }), &mut ctx).unwrap();
        assert!(bad.is_error);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn grep_ast_query_python() {
        let dir = std::env::temp_dir().join(format!("cc_astpy_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("m.py"), "def alpha():\n    pass\ndef beta(x):\n    return x\n").unwrap();
        let mut ctx = ToolCtx { root: &dir };
        let out = Grep
            .run(json!({ "ast_query": "(function_definition name: (identifier) @n)", "lang": "python" }), &mut ctx)
            .unwrap();
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("alpha") && out.content.contains("beta"), "{}", out.content);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn grep_bad_regex_errors() {
        let dir = tmp("bad");
        let mut ctx = ToolCtx { root: &dir };
        let out = Grep.run(json!({ "pattern": "(" }), &mut ctx).unwrap();
        assert!(out.is_error);
        std::fs::remove_dir_all(&dir).ok();
    }
}
