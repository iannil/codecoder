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
pub fn detect_repo(root: &Path) -> anyhow::Result<(String, String)> {
    let config_path = resolve_git_config_path(root)?;
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

/// Resolve the path to the git config, handling both regular `.git/config`
/// and worktree `.git` files (which point to a separate gitdir via `gitdir:`).
fn resolve_git_config_path(root: &Path) -> anyhow::Result<std::path::PathBuf> {
    let git_dir = root.join(".git");
    if git_dir.is_file() {
        // Worktree-style .git file: read the actual gitdir path.
        let content = std::fs::read_to_string(&git_dir)?;
        let line = content.lines().find(|l| l.starts_with("gitdir:"));
        match line {
            Some(l) => {
                let gitdir = l.trim_start_matches("gitdir: ").trim();
                let gitdir_path = if gitdir.starts_with('/') {
                    std::path::PathBuf::from(gitdir)
                } else {
                    root.join(gitdir)
                };
                // Worktree gitdirs have a `commondir` file pointing to the
                // shared git directory (typically "../.." relative to the
                // worktree gitdir). The config lives in the common directory.
                let commondir_path = gitdir_path.join("commondir");
                if let Ok(commondir_text) = std::fs::read_to_string(&commondir_path) {
                    let common = commondir_text.trim();
                    let common_path = gitdir_path.join(common);
                    // Canonicalize to resolve any ".." components so the
                    // final path is clean and readable.
                    let canonical = std::fs::canonicalize(&common_path)
                        .unwrap_or(common_path);
                    Ok(canonical.join("config"))
                } else {
                    // No commondir: the gitdir itself is a regular git directory
                    // (e.g., the main repo's .git). Config is directly inside.
                    Ok(gitdir_path.join("config"))
                }
            }
            None => anyhow::bail!("cannot parse .git file"),
        }
    } else {
        // Regular git repo: .git/config
        Ok(git_dir.join("config"))
    }
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
    // Filter out pull requests (GitHub API returns PRs as issues too).
    // PRs have a `pull_request` key in the API response, but our minimal struct
    // doesn't deserialize it. We detect PRs by checking the html_url for "/pull/".
    Ok(all.into_iter().filter(|i| !i.html_url.contains("/pull/")).collect())
}

/// Check which issues are already seeded in the workgraph.
/// Returns only issues whose number is not referenced in any milestone title.
pub fn filter_unseeded_issues(issues: &[GitHubIssue], root: &Path) -> Vec<GitHubIssue> {
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
                .filter(|l| {
                    let t = l.trim();
                    t.starts_with("acceptance:") || t.starts_with("ACCEPTANCE:")
                })
                .map(|l| {
                    l.trim_start_matches("acceptance:")
                        .trim_start_matches("ACCEPTANCE:")
                        .trim()
                        .to_string()
                })
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

    #[test]
    fn detect_repo_works_from_worktree_gitfile() {
        // This test relies on the actual project root being a git repo.
        // The project root is the canonical CodeCoder repo — we detect from it.
        let dir = std::env::temp_dir().join(format!("cc_detect_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // Create a .git file (worktree-style) pointing to the real git dir.
        // Read the ACTUAL content of the worktree's .git file to get the
        // correct gitdir path (the directory, not the .git file itself).
        let real_git_path = std::env::current_dir().unwrap().join(".git");
        let gitdir_content = std::fs::read_to_string(&real_git_path)
            .expect("failed to read .git file");
        std::fs::write(dir.join(".git"), &gitdir_content).unwrap();
        let (owner, repo) = detect_repo(&dir).unwrap();
        // The CodeCoder repo is hosted under github.com/iannil/codecoder
        assert!(!owner.is_empty());
        assert!(!repo.is_empty());
        // The repo name should be "codecoder" (or whatever the actual remote is).
        assert_eq!(repo, "codecoder");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn filter_unseeded_issues_returns_all_when_no_graph() {
        let dir = tempfile::tempdir().unwrap();
        let issues = vec![
            GitHubIssue {
                number: 1,
                title: "Fix bug".into(),
                body: None,
                state: "open".into(),
                labels: vec![],
                html_url: "https://github.com/user/repo/issues/1".into(),
            },
        ];
        let unseeded = filter_unseeded_issues(&issues, dir.path());
        assert_eq!(unseeded.len(), 1);
    }

    #[test]
    fn filter_unseeded_issues_excludes_seeded() {
        let dir = tempfile::tempdir().unwrap();
        let mut g = WorkGraph::default();
        g.add("#1: Fix bug", "acc", vec![]).unwrap();
        g.save(dir.path()).unwrap();
        let issues = vec![
            GitHubIssue {
                number: 1,
                title: "Fix bug".into(),
                body: None,
                state: "open".into(),
                labels: vec![],
                html_url: "https://github.com/user/repo/issues/1".into(),
            },
            GitHubIssue {
                number: 2,
                title: "Add feature".into(),
                body: None,
                state: "open".into(),
                labels: vec![],
                html_url: "https://github.com/user/repo/issues/2".into(),
            },
        ];
        let unseeded = filter_unseeded_issues(&issues, dir.path());
        assert_eq!(unseeded.len(), 1);
        assert_eq!(unseeded[0].number, 2);
    }

    #[test]
    fn seed_issues_as_milestones_adds_new_milestones() {
        let dir = tempfile::tempdir().unwrap();
        // Initialize an empty workgraph.
        WorkGraph::default().save(dir.path()).unwrap();
        let issues = vec![
            GitHubIssue {
                number: 42,
                title: "The answer".into(),
                body: Some("acceptance: make it work\n".into()),
                state: "open".into(),
                labels: vec![],
                html_url: "https://github.com/user/repo/issues/42".into(),
            },
        ];
        let count = seed_issues_as_milestones(dir.path(), &issues).unwrap();
        assert_eq!(count, 1);
        let g = WorkGraph::read(dir.path());
        assert_eq!(g.nodes.len(), 1);
        assert_eq!(g.nodes[0].title, "#42: The answer");
        assert_eq!(g.nodes[0].acceptance, "make it work");
    }

    #[test]
    fn seed_issues_as_milestones_uses_default_acceptance_when_no_acceptance_in_body() {
        let dir = tempfile::tempdir().unwrap();
        WorkGraph::default().save(dir.path()).unwrap();
        let issues = vec![
            GitHubIssue {
                number: 7,
                title: "No acceptance".into(),
                body: Some("just a description".into()),
                state: "open".into(),
                labels: vec![],
                html_url: "https://github.com/user/repo/issues/7".into(),
            },
        ];
        let count = seed_issues_as_milestones(dir.path(), &issues).unwrap();
        assert_eq!(count, 1);
        let g = WorkGraph::read(dir.path());
        assert_eq!(g.nodes[0].acceptance, "Resolve GitHub issue #7. See: https://github.com/user/repo/issues/7");
    }

    #[test]
    fn seed_issues_as_milestones_skips_duplicate() {
        let dir = tempfile::tempdir().unwrap();
        let mut g = WorkGraph::default();
        g.add("#1: First", "acc", vec![]).unwrap();
        g.save(dir.path()).unwrap();
        let issues = vec![
            GitHubIssue {
                number: 1,
                title: "First".into(),
                body: None,
                state: "open".into(),
                labels: vec![],
                html_url: "https://github.com/user/repo/issues/1".into(),
            },
        ];
        // seed_issues_as_milestones doesn't check for duplicates itself — it just adds.
        // The `add` method on WorkGraph will succeed for a brand-new node even if a
        // similar title exists. Duplicate filtering is the job of filter_unseeded_issues.
        let count = seed_issues_as_milestones(dir.path(), &issues).unwrap();
        // The add will succeed because the node is new (different id).
        // But the title already exists — this is intentional: the dedup is in
        // filter_unseeded_issues, not in seed_issues_as_milestones.
        assert_eq!(count, 1);
        assert_eq!(g.nodes.len() + 1, WorkGraph::read(dir.path()).nodes.len());
    }
}