# 0003 — WorkGraph Completion Acceptance

> Date: 2026-07-20

## What was done

Per the audit in `docs/audit/0002-first-class-citizen-analysis-2026-07-19.md` and the borrowing roadmap in `docs/adr/0027-pi-comparison-and-borrowing-roadmap.md`:

- **P0 (#4)**: Already in code — `src/review.rs` with structured verdicts (Verdict, Signals, parse_review, format_result)
- **P1 (#2)**: WorkGraph as a first-class citizen:
  - `src/workgraph.rs`: `WorkGraph`, `Milestone`, `NodeStatus` (incl. Hypothesis/Locked), `render_for_prompt`, `next_ready`, `recompute_blocked`, versioned persistence
  - `src/tool/dev.rs`: `milestone` tool (list/add/start/done/needs_fix/next/remove)
  - `src/agent.rs`: `drive_workgraph()` auto-advances ready milestones, parses review verdicts, auto-updates milestone status; `build_system_prompt()` includes `render_for_prompt()` output
  - `src/background.rs`: `resolve_bg_task()` falls back to workgraph, `run_background` loops through up to 3 milestones
- **P2 (#1)**: `NodeStatus` extended with `Hypothesis` and `Locked` variants for future inference-tree use
- **#6**: `SourceInfo` provenance metadata attached to all Registry entries (skills, prompts, capabilities)

## Test results

```
cargo test
```
160 passed, 0 failed, 4 ignored (pre-existing: 2 Docker e2e + L2 pty + L3 LLM smoke)

## Open items

- **P3 (#3)**: Inference/root-cause tree — deferred. Requires separate spec.
- **P4 (#5)**: Blueprint mirror — skill only, not in binary.
- **#7**: Hook/intercept middleware — deferred (conflicts with self-authoring safety loop ADR 0022).