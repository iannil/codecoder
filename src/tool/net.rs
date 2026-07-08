// Networking tools (ADR 0016 blocking I/O via ureq). Per CONTEXT.md's Sub-agent
// term these are Permission::None (read-only research), so sub-agents may use them.
use super::{Tool, ToolCtx, ToolOutput};
use crate::permission::Permission;
use serde_json::{Value, json};
use std::path::Path;

const MAX_BODY: usize = 8000;

fn http_get(url: &str) -> anyhow::Result<String> {
    match ureq::get(url).set("User-Agent", "codecoder").call() {
        Ok(r) => Ok(r.into_string()?),
        Err(ureq::Error::Status(code, r)) => {
            anyhow::bail!("HTTP {code}: {}", r.into_string().unwrap_or_default())
        }
        Err(e) => Err(anyhow::Error::new(e)),
    }
}

pub struct SearchWeb;

impl Tool for SearchWeb {
    fn name(&self) -> &str {
        "search_web"
    }
    fn description(&self) -> &str {
        "Fetch a URL and return its readable text content."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "url": { "type": "string" } },
            "required": ["url"]
        })
    }
    fn permission(&self, _args: &Value, _root: &Path) -> Permission {
        Permission::None
    }
    fn run(&self, args: Value, _ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
        let url = args.get("url").and_then(Value::as_str).unwrap_or_default();
        if url.is_empty() {
            return Ok(ToolOutput::err("missing required arg: url"));
        }
        match http_get(url) {
            Ok(body) => Ok(ToolOutput::ok(truncate(&strip_html(&body)))),
            Err(e) => Ok(ToolOutput::err(format!("fetch failed: {e}"))),
        }
    }
}

pub struct SearchGithub;

impl Tool for SearchGithub {
    fn name(&self) -> &str {
        "search_github"
    }
    fn description(&self) -> &str {
        "Search GitHub. Prefix the query with `repos:` (repositories) or `code:` (code)."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "query": { "type": "string", "description": "e.g. `repos:rust tui` or `code:fn main`" } },
            "required": ["query"]
        })
    }
    fn permission(&self, _args: &Value, _root: &Path) -> Permission {
        Permission::None
    }
    fn run(&self, args: Value, _ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
        let query = args.get("query").and_then(Value::as_str).unwrap_or_default();
        let (kind, q) = parse_github_query(query);
        if q.is_empty() {
            return Ok(ToolOutput::err("empty query"));
        }
        let url = format!("https://api.github.com/search/{kind}?q={}&per_page=10", urlencode(q));
        let mut req = ureq::get(&url)
            .set("User-Agent", "codecoder")
            .set("Accept", "application/vnd.github+json");
        if let Ok(token) = std::env::var("GITHUB_TOKEN") {
            req = req.set("Authorization", &format!("Bearer {token}"));
        }
        let json: Value = match req.call() {
            Ok(r) => r.into_json()?,
            Err(ureq::Error::Status(code, r)) => {
                return Ok(ToolOutput::err(format!("GitHub {code}: {}", r.into_string().unwrap_or_default())));
            }
            Err(e) => return Ok(ToolOutput::err(format!("request failed: {e}"))),
        };
        Ok(ToolOutput::ok(format_github(&json, kind)))
    }
}

pub struct ReverseApi;

impl Tool for ReverseApi {
    fn name(&self) -> &str {
        "reverse_api"
    }
    fn description(&self) -> &str {
        "Fetch a documentation page and extract likely HTTP API endpoints (method + path)."
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "url": { "type": "string" } },
            "required": ["url"]
        })
    }
    fn permission(&self, _args: &Value, _root: &Path) -> Permission {
        Permission::None
    }
    fn run(&self, args: Value, _ctx: &mut ToolCtx) -> anyhow::Result<ToolOutput> {
        let url = args.get("url").and_then(Value::as_str).unwrap_or_default();
        if url.is_empty() {
            return Ok(ToolOutput::err("missing required arg: url"));
        }
        match http_get(url) {
            Ok(body) => {
                let text = strip_html(&body);
                let endpoints = extract_endpoints(&text);
                if endpoints.is_empty() {
                    Ok(ToolOutput::ok("no endpoints detected"))
                } else {
                    Ok(ToolOutput::ok(endpoints.join("\n")))
                }
            }
            Err(e) => Ok(ToolOutput::err(format!("fetch failed: {e}"))),
        }
    }
}

// --- pure helpers (offline-testable) ---

fn parse_github_query(query: &str) -> (&'static str, &str) {
    let q = query.trim();
    if let Some(rest) = q.strip_prefix("code:") {
        ("code", rest.trim())
    } else if let Some(rest) = q.strip_prefix("repos:") {
        ("repositories", rest.trim())
    } else {
        ("repositories", q)
    }
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            b' ' => "+".to_string(),
            other => format!("%{other:02X}"),
        })
        .collect()
}

fn format_github(json: &Value, kind: &str) -> String {
    let Some(items) = json.get("items").and_then(Value::as_array) else {
        return "no results".into();
    };
    let mut out = Vec::new();
    for it in items {
        if kind == "repositories" {
            let name = it.get("full_name").and_then(Value::as_str).unwrap_or("?");
            let desc = it.get("description").and_then(Value::as_str).unwrap_or("");
            let stars = it.get("stargazers_count").and_then(Value::as_u64).unwrap_or(0);
            out.push(format!("{name} ★{stars} — {desc}"));
        } else {
            let repo = it.get("repository").and_then(|r| r.get("full_name")).and_then(Value::as_str).unwrap_or("?");
            let path = it.get("path").and_then(Value::as_str).unwrap_or("?");
            out.push(format!("{repo}: {path}"));
        }
    }
    if out.is_empty() { "no results".into() } else { out.join("\n") }
}

/// Very small HTML→text: drop <script>/<style> bodies, strip tags, collapse blanks.
fn strip_html(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let bytes = html.as_bytes();
    let lower = html.to_ascii_lowercase();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'<' {
            // Skip whole <script>/<style> blocks including their content.
            for (tag, end) in [("<script", "</script>"), ("<style", "</style>")] {
                if lower[i..].starts_with(tag) {
                    if let Some(pos) = lower[i..].find(end) {
                        i += pos + end.len();
                    } else {
                        i = bytes.len();
                    }
                    continue;
                }
            }
            // Otherwise skip to the closing '>'.
            match html[i..].find('>') {
                Some(pos) => i += pos + 1,
                None => break,
            }
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    // Collapse runs of blank lines / whitespace.
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate(s: &str) -> String {
    if s.len() <= MAX_BODY {
        s.to_string()
    } else {
        format!("{}… [{} more bytes]", &s[..MAX_BODY], s.len() - MAX_BODY)
    }
}

/// Heuristic endpoint extraction: `METHOD /path` occurrences, de-duplicated.
fn extract_endpoints(text: &str) -> Vec<String> {
    const METHODS: [&str; 5] = ["GET", "POST", "PUT", "DELETE", "PATCH"];
    let tokens: Vec<&str> = text.split_whitespace().collect();
    let mut seen = std::collections::BTreeSet::new();
    for w in tokens.windows(2) {
        let m = w[0].trim_matches(|c: char| !c.is_ascii_alphabetic());
        if METHODS.contains(&m) && w[1].starts_with('/') {
            let path = w[1].trim_matches(|c: char| c == ',' || c == '"' || c == '`' || c == ')');
            seen.insert(format!("{m} {path}"));
        }
    }
    seen.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_query_prefixes() {
        assert_eq!(parse_github_query("repos:rust tui"), ("repositories", "rust tui"));
        assert_eq!(parse_github_query("code:fn main"), ("code", "fn main"));
        assert_eq!(parse_github_query("just words"), ("repositories", "just words"));
    }

    #[test]
    fn strips_script_and_tags() {
        let html = "<html><script>var x=1<2;</script><body><p>Hello <b>world</b></p></body></html>";
        assert_eq!(strip_html(html), "Hello world");
    }

    #[test]
    fn extracts_endpoints() {
        let text = "The API: GET /users/{id} returns a user. POST /users creates one. GET /users/{id} again.";
        let eps = extract_endpoints(text);
        assert!(eps.contains(&"GET /users/{id}".to_string()));
        assert!(eps.contains(&"POST /users".to_string()));
        assert_eq!(eps.len(), 2); // de-duplicated
    }

    #[test]
    fn urlencodes_spaces_and_specials() {
        assert_eq!(urlencode("a b/c"), "a+b%2Fc");
    }
}
