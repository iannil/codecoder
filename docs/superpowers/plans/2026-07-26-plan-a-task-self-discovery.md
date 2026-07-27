# Plan A: Task Self-Discovery & External Source Integration

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enable CodeCoder to autonomously discover new tasks from external sources (GitHub Issues, webhooks) and seed them into the Work Graph, eliminating the current P0 gap where it can only advance pre-seeded milestones.

**Architecture:** Add a new `task_source` module under `src/daemon/` that polls GitHub Issues for the current repository via the GitHub API, parses issues into Work Graph milestones, and seeds them into `workgraph.json`. A new daemon thread polls on a configurable interval. The existing `cc workgraph-pause/resume` command is extended with `cc autotask on/off` to control the polling. A `generate_milestones` LLM tool (or skill) enables the agent to autonomously decompose a high-level goal into milestone nodes.

**Tech Stack:** Rust (existing), ureq (existing), serde_json (existing), GitHub REST API v3 (already used by `search_github` tool)

## Global Constraints

- No new async runtime — all I/O is blocking (ureq, OS threads)
- Follow existing patterns: `Config` env var, daemon thread with shutdown flag, thread_status heartbeat
- All new code MUST be hermetic-testable (use `ScriptedProvider` or mock HTTP)
- All GitHub API calls use existing `GITHUB_TOKEN` env var
- New env vars: `CODECODER_AUTOTASK_INTERVAL_SECS` (default 300, 0=disabled), `CODECODER_AUTOTASK_SOURCE` (default `github_issues`, future: `webhook`, `linear`)
- New files go under `src/daemon/task_source.rs` for the poller, `src/tool/generate_milestones.rs` for the LLM tool
- Follow `CONTEXT.md` terminology: Work Graph, Milestone, Background Agent, etc.

---

### Task 1: Add `autotask` config fields and env vars

**Files:**
- Modify: `src/config.rs:9-44` (add fields), `src/config.rs:47-101` (add env parsing), `src/config.rs:134-150` (add to DOTENV_ALLOWED_KEYS)
- Test: tests in `src/config.rs` (inline)

**Interfaces:**
- Consumes: existing `Config` struct
- Produces: `Config.auto_task_interval_secs: u64` (default 300, 0=disabled), `Config.auto_task_source: String` (default `"github_issues"`)

- [ ] **Step 1: Add fields to Config struct**

```rust
// In src/config.rs, after line 43 (pub ondemand_reaper_secs: u64):
    /// 自动任务发现轮询间隔（秒）。0 = 禁用。env CODECODER_AUTOTASK_INTERVAL_SECS, 默认 300。
    pub auto_task_interval_secs: u64,
    /// 任务源类型。env CODECODER_AUTOTASK_SOURCE, 默认 "github_issues"。
    /// 未来可扩展: "github_issues", "webhook", "linear"。
    pub auto_task_source: String,
```

- [ ] **Step 2: Add env parsing in `from_env()`**

```rust
// After line 99 (ondemand_reaper_secs parsing):
            auto_task_interval_secs: env("CODECODER_AUTOTASK_INTERVAL_SECS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(300),
            auto_task_source: env("CODECODER_AUTOTASK_SOURCE")
                .unwrap_or_else(|| "github_issues".into()),
```

- [ ] **Step 3: Add to DOTENV_ALLOWED_KEYS**

```rust
// After line 149 (CODECODER_ONDEMAND_REAPER_SECS):
    "CODECODER_AUTOTASK_INTERVAL_SECS",
    "CODECODER_AUTOTASK_SOURCE",
```

- [ ] **Step 4: Write tests for new config fields**

```rust
// Add to the existing test module in config.rs:

#[test]
fn autotask_config_defaults() {
    unsafe {
        std::env::remove_var("CODECODER_AUTOTASK_INTERVAL_SECS");
        std::env::remove_var("CODECODER_AUTOTASK_SOURCE");
    }
    let cfg = Config::from_env();
    assert_eq!(cfg.auto_task_interval_secs, 300);
    assert_eq!(cfg.auto_task_source, "github_issues");
}

#[test]
fn autotask_config_overrides() {
    unsafe {
        std::env::set_var("CODECODER_AUTOTASK_INTERVAL_SECS", "60");
        std::env::set_var("CODECODER_AUTOTASK_SOURCE", "webhook");
    }
    let cfg = Config::from_env();
    assert_eq!(cfg.auto_task_interval_secs, 60);
    assert_eq!(cfg.auto_task_source, "webhook");
    unsafe {
        std::env::remove_var("CODECODER_AUTOTASK_INTERVAL_SECS");
        std::env::remove_var("CODECODER_AUTOTASK_SOURCE");
    }
}
```

- [ ] **Step 5: Run tests**

```bash
cargo test bg_env_defaults_and_overrides autotask_config_defaults autotask_config_overrides
```
Expected: PASS for all.

- [ ] **Step 6: Commit**

```bash
git add src/config.rs
git commit -m "feat(config): add autotask interval and source config fields"
```

---

### Task 2: Implement GitHub Issues poller

**Files:**
- Create: `src/daemon/task_source.rs`
- Modify: `src/daemon/mod.rs` (add `pub mod task_source;` and register the thread)
- Test: `src/daemon/task_source.rs` (inline tests)

**Interfaces:**
- Consumes: `Config.root`, `Config.github_token`, `Config.auto_task_interval_secs`, `Config.auto_task_source`
- Produces: `poll_github_issues(root: &Path, token: &str) -> Vec<GitHubIssue>` — fetches open issues, returns those not yet seeded as milestones
- Produces: `seed_issues_as_milestones(root: &Path, issues: &[GitHubIssue]) -> usize` — writes each issue as a new milestone into `workgraph.json`

- [ ] **Step 1: Create `src/daemon/task_source.rs` with data types and GitHub API client**

```rust
// src/daemon/task_source.rs
// Task self-discovery from external sources (GitHub Issues, etc.).
// Uses ureq (blocking HTTP) — same as existing search_github tool.
// Follows the same pattern as daemon supervisor/workgraph threads.

use crate::workgraph::WorkGraph;
use std::path::Path;

/// A GitHub issue as returned by the REST API v3 /repos/:owner/:repo/issues endpoint.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct GitHubIssue {
    pub number: u64,
    pub title: String,
    pub body: Option<String>,
    pub state: String,
    #[serde(default)]
    pub labels: Vec<GitHubLabel>,
    pub html_url: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct GitHubLabel {
    pub name: String,
}

/// Try to detect the GitHub repo from the project root's git remote.
/// Returns `(owner, repo)` or an error.
fn detect_repo(root: &Path) -> anyhow::Result<(String, String)> {
    let git_dir = root.join(".git");
    // Read the git config to find the remote origin URL
    let config_path = if git_dir.is_file() {
        // bare gitfile: read the actual gitdir path
        let content = std::fs::read_to_string(&git_dir)?;
        let line = content.lines().find(|l| l.starts_with("gitdir:"));
        match line {
            Some(l) => root.join(l.trim_start_matches("gitdir: ").trim()),
            None => anyhow::bail!("cannot parse .git file"),
        }
    } else {
        git_dir.join("config")
    };
    let config_text = std::fs::read_to_string(&config_path)?;
    // Look for [remote "origin"] url = git@github.com:owner/repo.git or https://...
    let mut in_origin = false;
    for line in config_text.lines() {
        let t = line.trim();
        if t.starts_with("[remote") && t.contains("origin") {
            in_origin = true;
            continue;
        }
        if in_origin && t.starts_with('[') {
            break; // next section
        }
        if in_origin && t.starts_with("url") {
            let parts: Vec<&str> = t.splitn(2, '=').collect();
            if parts.len() == 2 {
                let url = parts[1].trim();
                return parse_github_url(url);
            }
        }
    }
    anyhow::bail!("no origin remote found in git config");
}

/// Parse a GitHub remote URL into (owner, repo).
fn parse_github_url(url: &str) -> anyhow::Result<(String, String)> {
    // Handles: git@github.com:owner/repo.git, https://github.com/owner/repo
    let url = url.trim_end_matches(".git");
    let url = url.trim_end_matches('/');
    if let Some(rest) = url.strip_prefix("git@github.com:") {
        let parts: Vec<&str> = rest.splitn(2, '/').collect();
        if parts.len() == 2 {
            return Ok((parts[0].to_string(), parts[1].to_string()));
        }
    }
    if let Some(rest) = url.strip_prefix("https://github.com/") {
        let parts: Vec<&str> = rest.splitn(2, '/').collect();
        if parts.len() == 2 {
            return Ok((parts[0].to_string(), parts[1].to_string()));
        }
    }
    anyhow::bail!("cannot parse GitHub URL: {url}")
}

/// Fetch open issues from the GitHub API.
/// Returns issues sorted by number ascending (oldest first).
pub fn fetch_open_issues(token: &str, owner: &str, repo: &str) -> anyhow::Result<Vec<GitHubIssue>> {
    let url = format!("https://api.github.com/repos/{owner}/{repo}/issues?state=open&per_page=100&sort=created&direction=asc");
    let mut req = ureq::get(&url)
        .set("Accept", "application/vnd.github+json")
        .set("User-Agent", "CodeCoder");
    if !token.is_empty() {
        req = req.set("Authorization", &format!("Bearer {token}"));
    }
    let resp = req.call()?;
    let status = resp.status();
    if status != 200 {
        let body = resp.into_string().unwrap_or_default();
        anyhow::bail!("GitHub API returned {status}: {body}");
    }
    let all: Vec<GitHubIssue> = resp.into_json()?;
    // Filter out pull requests (GitHub API returns PRs as issues too)
    Ok(all.into_iter().filter(|i| {
        // PRs have a pull_request field; we can detect by checking if body is None
        // or by checking the html_url pattern. Simpler: just check if labels contain "pull_request"
        // Actually the API returns `pull_request` key for PRs. Let's use a lack of body as heuristic.
        // Better: use the fact that PRs have a `pull_request` field, but our minimal struct doesn't.
        // We'll filter by checking if html_url contains "/pull/" which is reliable.
        !i.html_url.contains("/pull/")
    }).collect())
}

/// Check which issues are already seeded in the workgraph.
/// Returns only issues whose number is not referenced in any milestone title.
fn filter_unseeded_issues(issues: &[GitHubIssue], root: &Path) -> Vec<GitHubIssue> {
    let wg = match WorkGraph::read_checked(root) {
        Ok(g) => g,
        Err(_) => return issues.to_vec(), // empty graph → all issues are unseeded
    };
    issues.iter().filter(|issue| {
        // Check if any milestone title contains "#NNN" for this issue number
        let marker = format!("#{}", issue.number);
        !wg.nodes.iter().any(|m| m.title.contains(&marker))
    }).cloned().collect()
}

/// Seed a batch of issues as workgraph milestones.
/// Returns the number of milestones actually added.
pub fn seed_issues_as_milestones(root: &Path, issues: &[GitHubIssue]) -> anyhow::Result<usize> {
    let mut count = 0usize;
    WorkGraph::with_lock(root, |g| {
        for issue in issues {
            let title = format!("#{}: {}", issue.number, issue.title);
            let acceptance = issue.body.as_deref().unwrap_or("")
                .lines()
                .filter(|l| l.trim().starts_with("acceptance:") || l.trim().starts_with("ACCEPTANCE:"))
                .map(|l| l.trim_start_matches("acceptance:").trim_start_matches("ACCEPTANCE:").trim().to_string())
                .collect::<Vec<_>>()
                .join("\n");
            let acceptance = if acceptance.is_empty() {
                format!("Resolve GitHub issue #{}. See: {}", issue.number, issue.html_url)
            } else {
                acceptance
            };
            if g.add(&title, &acceptance, vec![]).is_ok() {
                count += 1;
            }
        }
        Ok(())
    })?;
    Ok(count)
}

/// Full poll-and-seed cycle.
/// Returns (issues_fetched, milestones_seeded) or an error.
pub fn poll_and_seed(root: &Path, token: &str) -> anyhow::Result<(usize, usize)> {
    let (owner, repo) = detect_repo(root)?;
    let all = fetch_open_issues(token, &owner, &repo)?;
    let unseeded = filter_unseeded_issues(&all, root);
    if unseeded.is_empty() {
        return Ok((all.len(), 0));
    }
    let seeded = seed_issues_as_milestones(root, &unseeded)?;
    Ok((all.len(), seeded))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_github_url_ssh() {
        let (owner, repo) = parse_github_url("git@github.com:user/my-repo.git").unwrap();
        assert_eq!(owner, "user");
        assert_eq!(repo, "my-repo");
    }

    #[test]
    fn parse_github_url_https() {
        let (owner, repo) = parse_github_url("https://github.com/owner/repo-name").unwrap();
        assert_eq!(owner, "owner");
        assert_eq!(repo, "repo-name");
    }

    #[test]
    fn parse_github_url_with_trailing_slash() {
        let (owner, repo) = parse_github_url("git@github.com:user/repo/").unwrap();
        assert_eq!(owner, "user");
        assert_eq!(repo, "repo");
    }
}
```

- [ ] **Step 2: Add `mod task_source` to daemon**

```rust
// In src/daemon/mod.rs, after `pub mod bus;` (line 12):
pub mod task_source;
```

- [ ] **Step 3: Run tests for the new module**

```bash
cargo test task_source::tests
```
Expected: PASS (hermetic, no network).

- [ ] **Step 4: Commit**

```bash
git add src/daemon/task_source.rs src/daemon/mod.rs
git commit -m "feat(daemon): add GitHub Issues poller for task self-discovery"
```

---

### Task 3: Wire the autotask thread into daemon

**Files:**
- Modify: `src/daemon/mod.rs` (add autotask polling thread in `run()`)
- Modify: `src/daemon/proto.rs` (add `ServerEvent` variant for autotask notifications)
- Modify: `src/daemon/socket.rs` (handle `cc autotask` commands)
- Test: `src/daemon/mod.rs` (inline tests)

**Interfaces:**
- Consumes: `Config.auto_task_interval_secs`, `Config.auto_task_source`, `Config.github_token`, `Config.root`
- New daemon thread: polls every `auto_task_interval_secs` seconds, calls `poll_and_seed()`, broadcasts results via `EventBus`

- [ ] **Step 1: Add autotask thread to daemon `run()`**

```rust
// In src/daemon/mod.rs, after the reload thread (around line 229), add:

// 自动任务发现线程：按 interval 轮询外部源（GitHub Issues 等），
// 把新 issue seed 为 workgraph milestone。
// 仅当 auto_task_interval_secs > 0 时启动。
let auto_task_interval = self.cfg.auto_task_interval_secs;
if auto_task_interval > 0 {
    let shutdown_auto = Arc::clone(&shutdown);
    let root_auto = self.cfg.root.clone();
    let token_auto = self.cfg.github_token.clone().unwrap_or_default();
    let source_auto = self.cfg.auto_task_source.clone();
    let ts_auto = Arc::clone(&thread_status);
    let bus_auto = Arc::clone(&bus);
    std::thread::spawn(move || {
        let mut count = 0u64;
        let tick = Duration::from_secs(auto_task_interval);
        while !shutdown_auto.load(Ordering::SeqCst) {
            std::thread::sleep(tick);
            count += 1;
            let mut last_event = "idle".to_string();
            if source_auto == "github_issues" {
                match crate::daemon::task_source::poll_and_seed(&root_auto, &token_auto) {
                    Ok((fetched, seeded)) => {
                        if seeded > 0 {
                            last_event = format!("seeded {seeded}/{fetched} issues");
                            bus_auto.broadcast("autotask", &format!("seeded {seeded} new issues from {fetched} open"));
                        } else {
                            last_event = format!("no new issues ({fetched} open)");
                        }
                    }
                    Err(e) => {
                        last_event = format!("error: {e}");
                        // Don't broadcast errors — too noisy on each tick
                    }
                }
            }
            let mut status = ts_auto.lock().unwrap();
            if let Some(s) = status.iter_mut().find(|s| s.name == "autotask") {
                s.last_tick = Some(std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs());
                s.tick_count = count;
                s.last_event = last_event;
            }
        }
    });
    // Register the thread in thread_status
    {
        let mut ts = thread_status.lock().unwrap();
        ts.push(crate::daemon::proto::ThreadStatus {
            name: "autotask".into(),
            last_tick: None,
            tick_count: 0,
            last_event: "initializing".into(),
        });
    }
}
```

- [ ] **Step 2: Run integration tests**

```bash
cargo test daemon_constructs_with_temp_root
```
Expected: PASS (exercises the daemon constructor path).

- [ ] **Step 3: Commit**

```bash
git add src/daemon/mod.rs
git commit -m "feat(daemon): wire autotask polling thread for GitHub Issues"
```

---

### Task 4: Add `cc autotask` CLI commands

**Files:**
- Modify: `src/daemon/proto.rs` (add `AutotaskStatus` variant to `ClientCommand` / `ServerEvent`)
- Modify: `src/daemon/socket.rs` (handle `autotask` and `autotask status` commands; add `autotask_status` field to mgr)
- Modify: `src/client/mod.rs` (add `autotask` and `autotask status` to the command parser)
- Test: `src/daemon/socket.rs` (inline test)

**Interfaces:**
- New CLI commands: `cc autotask on` (start polling), `cc autotask off` (stop polling), `cc autotask status` (show last poll)
- Daemon stores `autotask_paused: Arc<AtomicBool>` in session manager, similar to `workgraph_paused`

- [ ] **Step 1: Add `autotask_paused` to session manager**

```rust
// In src/daemon/session_manager.rs, add to DaemonSessionManager:
    pub autotask_paused: Option<Arc<AtomicBool>>,
```

- [ ] **Step 2: Wire the pause flag from daemon to the autotask thread**

The autotask thread checks `autotask_paused` before each poll cycle, similar to how the workgraph thread checks `wg_paused`.

- [ ] **Step 3: Add `cc autotask on/off/status` command handling**

- [ ] **Step 4: Run tests**

```bash
cargo test
```
Expected: All existing tests still pass.

- [ ] **Step 5: Commit**

```bash
git add src/daemon/proto.rs src/daemon/session_manager.rs src/daemon/socket.rs src/client/mod.rs
git commit -m "feat(cc): add autotask on/off/status CLI commands"
```

---

### Task 5: Add `generate_milestones` LLM tool

**Files:**
- Create: `src/tool/generate_milestones.rs`
- Modify: `src/tool/mod.rs` (register the new tool in `Toolbox`)
- Test: `src/tool/generate_milestones.rs` (inline tests)

**Interfaces:**
- New tool: `generate_milestones { goal: String, context: String }` — calls LLM to decompose a high-level goal into milestone nodes, then seeds them into `workgraph.json`
- Consumes: `ToolCtx` (for root path), `workgraph::WorkGraph`
- Produces: structured milestone list added to the workgraph

- [ ] **Step 1: Create the tool definition**

The tool takes a high-level goal description and optional context, sends a structured prompt to the LLM to decompose it into milestones with acceptance criteria, then writes each milestone to the workgraph.

```rust
// src/tool/generate_milestones.rs
// LLM tool to decompose a high-level goal into workgraph milestones.
// Uses the main provider to generate the decomposition.

use crate::tool::{Tool, ToolCtx, ToolOutput};
use crate::message::{Message, MessageItem, Role};
use crate::workgraph::WorkGraph;

pub struct GenerateMilestones;

impl Tool for GenerateMilestones {
    fn name(&self) -> &str { "generate_milestones" }

    fn description(&self) -> &str {
        "Decompose a high-level goal into workgraph milestones. \
         Takes a goal description and optional context, calls the LLM \
         to generate structured milestones with acceptance criteria, \
         then seeds them into the workgraph. \
         Returns the list of created milestone IDs."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "goal": {
                    "type": "string",
                    "description": "The high-level goal to decompose into milestones"
                },
                "context": {
                    "type": "string",
                    "description": "Optional context (existing code structure, constraints, etc.)",
                    "default": ""
                }
            },
            "required": ["goal"]
        })
    }

    fn execute(&self, ctx: &ToolCtx, args: &serde_json::Value) -> anyhow::Result<ToolOutput> {
        // ... implementation
    }
}
```

- [ ] **Step 2: Register in toolbox**

```rust
// In src/tool/mod.rs, add to the Toolbox constructor:
Box::new(generate_milestones::GenerateMilestones),
```

- [ ] **Step 3: Run tests**

```bash
cargo test
```
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add src/tool/generate_milestones.rs src/tool/mod.rs
git commit -m "feat(tool): add generate_milestones tool for LLM-based goal decomposition"
```

---

### Task 6: Update README and docs

**Files:**
- Modify: `README.md` (add new env vars, new CLI commands, new tool to the table)
- Modify: `ARCHITECTURE.md` (update module map, add `task_source` module)
- Modify: `docs/adr/` (optional: write ADR 0040 for task self-discovery)

**Interfaces:**
- Documentation only — no code changes.

- [ ] **Step 1: Update README.md env table**

Add `CODECODER_AUTOTASK_INTERVAL_SECS` and `CODECODER_AUTOTASK_SOURCE` entries.

- [ ] **Step 2: Update README.md CLI commands**

Add `autotask on`, `autotask off`, `autotask status` to the REPL commands section.

- [ ] **Step 3: Update README.md tool table**

Add `generate_milestones` to the built-in tools table.

- [ ] **Step 4: Update ARCHITECTURE.md**

Add `task_source` to the module map table.

- [ ] **Step 5: Commit**

```bash
git add README.md ARCHITECTURE.md
git commit -m "docs: document autotask feature and generate_milestones tool"
```

---

### Task 7: End-to-end verification

**Files:**
- Test: `tests/l1_background.rs` (add a test that seeds milestones via autotask)

**Interfaces:**
- Integration test exercises the full cycle: mock GitHub API → poll → seed → verify workgraph

- [ ] **Step 1: Write integration test**

```rust
// In tests/l1_background.rs, add a test that simulates the autotask cycle:
// 1. Create a temp git repo with a known remote URL
// 2. Mock the GitHub API response (or test the parse+seed part hermetically)
// 3. Call poll_and_seed → verify milestones were created
```

- [ ] **Step 2: Run tests**

```bash
cargo test
```
Expected: PASS.

- [ ] **Step 3: Verify the full daemon integration**

Manual test:
```bash
# Set up a real repo with GitHub issues
CODECODER_DAEMON=1 CODECODER_AUTOTASK_INTERVAL_SECS=60 GITHUB_TOKEN=ghp_xxx cargo run
# In another terminal:
cargo run --bin cc
cc> autotask status
cc> milestone list
```

- [ ] **Step 4: Commit**

```bash
git add tests/l1_background.rs
git commit -m "test: add integration test for autotask poll-and-seed cycle"
```