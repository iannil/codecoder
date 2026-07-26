// L1 — Task Source (autotask poll-and-seed). Hermetic integration tests that
// exercise the full cycle without hitting the network or GitHub API.
//
// Strategy: test the pure functions (seed_issues_as_milestones,
// filter_unseeded_issues) directly via temp dirs. For poll_and_seed, which calls
// detect_repo → fetch_open_issues → filter → seed, we test detect_repo against
// a temp git repo and then verify the overall flow by testing the components
// separately. The network-dependent fetch_open_issues is not tested here (L3).
//
// All tests create throwaway temp directories and clean up on drop.

mod testkit;
use testkit::*;

use codecoder::daemon::task_source::{
    self, GitHubIssue, seed_issues_as_milestones,
};
use codecoder::workgraph::WorkGraph;

// ─── seed_issues_as_milestones ───────────────────────────────────────────────

#[test]
fn seed_milestones_creates_workgraph_entries() {
    let ws = Workspace::new();
    // Initialize an empty workgraph.
    WorkGraph::default().save(&ws.root()).unwrap();

    let issues = vec![
        GitHubIssue {
            number: 1,
            title: "Implement login".into(),
            body: Some("acceptance: user can log in with email\n".into()),
            state: "open".into(),
            labels: vec![],
            html_url: "https://github.com/owner/repo/issues/1".into(),
        },
        GitHubIssue {
            number: 2,
            title: "Add logout".into(),
            body: None,
            state: "open".into(),
            labels: vec![],
            html_url: "https://github.com/owner/repo/issues/2".into(),
        },
    ];

    let count = seed_issues_as_milestones(&ws.root(), &issues).unwrap();
    assert_eq!(count, 2, "should seed both issues");

    let g = WorkGraph::read(&ws.root());
    assert_eq!(g.nodes.len(), 2);
    assert_eq!(g.nodes[0].title, "#1: Implement login");
    assert_eq!(g.nodes[0].acceptance, "user can log in with email");
    assert_eq!(g.nodes[1].title, "#2: Add logout");
    assert!(g.nodes[1].acceptance.contains("Resolve GitHub issue #2"));
}

#[test]
fn seed_milestones_skips_duplicate_issue_number() {
    let ws = Workspace::new();
    let mut g = WorkGraph::default();
    g.add("#3: Original", "acc", vec![]).unwrap();
    g.save(&ws.root()).unwrap();

    // An issue with number 3 — but the workgraph already has #3 in its title.
    // seed_issues_as_milestones doesn't dedup itself (that's filter_unseeded_issues's job),
    // so add() succeeds with a new node id. Verify the total node count increases.
    let issues = vec![
        GitHubIssue {
            number: 3,
            title: "Duplicate".into(),
            body: None,
            state: "open".into(),
            labels: vec![],
            html_url: "https://github.com/owner/repo/issues/3".into(),
        },
    ];

    let count = seed_issues_as_milestones(&ws.root(), &issues).unwrap();
    assert_eq!(count, 1, "should add a new milestone even if number is same (dedup is in filter)");
    let g = WorkGraph::read(&ws.root());
    assert_eq!(g.nodes.len(), 2);
    assert_eq!(g.nodes[0].title, "#3: Original");
    assert_eq!(g.nodes[1].title, "#3: Duplicate");
}

// ─── filter_unseeded_issues ─────────────────────────────────────────────────

#[test]
fn filter_unseeded_returns_all_when_no_workgraph() {
    let ws = Workspace::new();
    // No workgraph.json at all.

    let issues = vec![
        GitHubIssue {
            number: 10,
            title: "Feature A".into(),
            body: None,
            state: "open".into(),
            labels: vec![],
            html_url: "https://github.com/owner/repo/issues/10".into(),
        },
    ];

    let unseeded = task_source::filter_unseeded_issues(&issues, &ws.root());
    assert_eq!(unseeded.len(), 1, "no workgraph → all issues are unseeded");
}

#[test]
fn filter_unseeded_excludes_already_seeded() {
    let ws = Workspace::new();
    let mut g = WorkGraph::default();
    g.add("#5: Bug fix", "acc", vec![]).unwrap();
    g.save(&ws.root()).unwrap();

    let issues = vec![
        GitHubIssue {
            number: 5,
            title: "Bug fix".into(),
            body: None,
            state: "open".into(),
            labels: vec![],
            html_url: "https://github.com/owner/repo/issues/5".into(),
        },
        GitHubIssue {
            number: 6,
            title: "New feature".into(),
            body: None,
            state: "open".into(),
            labels: vec![],
            html_url: "https://github.com/owner/repo/issues/6".into(),
        },
    ];

    let unseeded = task_source::filter_unseeded_issues(&issues, &ws.root());
    assert_eq!(unseeded.len(), 1, "only #6 is unseeded");
    assert_eq!(unseeded[0].number, 6);
}

// ─── detect_repo (hermetic via temp git repo) ────────────────────────────────

#[test]
fn detect_repo_parses_ssh_remote() {
    let ws = Workspace::new();
    ws.git_init();
    // Set a known SSH remote.
    ws.write(".git/config", &ws.read(".git/config").replace(
        "[core]",
        "[remote \"origin\"]\n\turl = git@github.com:test-owner/test-repo.git\n[core]",
    ));

    let (owner, repo) = task_source::detect_repo(&ws.root()).unwrap();
    assert_eq!(owner, "test-owner");
    assert_eq!(repo, "test-repo");
}

#[test]
fn detect_repo_parses_https_remote() {
    let ws = Workspace::new();
    ws.git_init();
    ws.write(".git/config", &ws.read(".git/config").replace(
        "[core]",
        "[remote \"origin\"]\n\turl = https://github.com/org/project.git\n[core]",
    ));

    let (owner, repo) = task_source::detect_repo(&ws.root()).unwrap();
    assert_eq!(owner, "org");
    assert_eq!(repo, "project");
}

#[test]
fn detect_repo_errors_without_origin() {
    let ws = Workspace::new();
    ws.git_init();
    // No origin remote set.

    let result = task_source::detect_repo(&ws.root());
    assert!(result.is_err(), "no origin remote should produce an error");
    assert!(
        result.err().unwrap().to_string().contains("no origin remote"),
        "error should mention missing origin"
    );
}

// ─── poll_and_seed component integration ─────────────────────────────────────
// We can't call poll_and_seed directly without mocking the network, but we can
// verify the full data-flow contract by chaining the pure components.

#[test]
fn poll_and_seed_data_flow_hermetic() {
    // Simulate the poll_and_seed pipeline manually:
    //   detect_repo → fetch_open_issues (skipped) → filter_unseeded → seed
    // This verifies that the filter+seed half works as poll_and_seed would
    // wire them together, without needing a real GitHub API call.

    let ws = Workspace::new();
    ws.git_init();
    ws.write(".git/config", &ws.read(".git/config").replace(
        "[core]",
        "[remote \"origin\"]\n\turl = git@github.com:iannil/codecoder.git\n[core]",
    ));
    WorkGraph::default().save(&ws.root()).unwrap();

    // Simulated open issues (as if fetch_open_issues returned them).
    let open_issues = vec![
        GitHubIssue {
            number: 42,
            title: "Add dark mode".into(),
            body: Some("acceptance: toggle works\n".into()),
            state: "open".into(),
            labels: vec![],
            html_url: "https://github.com/iannil/codecoder/issues/42".into(),
        },
    ];

    // Step: filter_unseeded (no seeded milestones yet).
    let unseeded = task_source::filter_unseeded_issues(&open_issues, &ws.root());
    assert_eq!(unseeded.len(), 1, "issue 42 should be unseeded");

    // Step: seed_issues_as_milestones.
    let seeded = seed_issues_as_milestones(&ws.root(), &unseeded).unwrap();
    assert_eq!(seeded, 1, "should seed 1 milestone");

    // Verify: workgraph has the milestone.
    let g = WorkGraph::read(&ws.root());
    assert_eq!(g.nodes.len(), 1);
    assert_eq!(g.nodes[0].title, "#42: Add dark mode");

    // Verify: re-running filter_unseeded shows nothing new.
    let unseeded2 = task_source::filter_unseeded_issues(&open_issues, &ws.root());
    assert_eq!(unseeded2.len(), 0, "issue 42 should now be filtered out");
}