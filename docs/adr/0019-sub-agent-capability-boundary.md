# Sub-agent capability boundary = a curated side-effect-free tool set

A sub-agent's "read-only by contract" is given a precise, enforced meaning: **its tool set is a curated subset of the tools returning `Permission::None`** — the side-effect-free ones (`read_file`, `list_directory`, `glob`, `grep`, `search_web`, `search_github`, `reverse_api`, `use_skill`, `diff`), plus no `ask_user`. This falls out of a structural fact rather than a policy choice: a sub-agent has no user to answer a permission prompt, so any tool that would prompt is simply absent.

## Why "curated subset" and not "all Permission::None"

`Permission::None` gates **prompting**; it is not a guarantee of no side effects. A few tools never prompt yet **persist state to disk**, which would violate the read-only intent of delegation and are therefore deliberately excluded from `Toolbox::read_only_child()`:

- `reason` writes `causal_tree.json`,
- `milestone` writes `workgraph.json`,
- `memory` writes the persistent key-value store,
- and any write/execute tool (which prompts anyway).

The boundary the sub-agent enforces is "no prompt AND no persistent side effect," not merely "no prompt."

## Why it's forced, not chosen

Only the top-level agent owns the user-facing channel (see [[0016-channel-topology-and-event-model]]). A sub-agent therefore has no one who could answer a permission prompt. Rather than bubble prompts up to the parent — which would make a user field permission dialogs for work they didn't directly order, blurring accountability — permission-requiring tools are simply **not in the sub-agent's set**. The permission question is eliminated because the sub-agent cannot reach any tool that would raise it.

## Consequences

- A sub-agent runs on its own thread that the parent's `agent`-tool call joins; it reports its result back to the parent as that tool's `ToolOutput`, never to the user.
- It **cannot spawn further sub-agents** (depth locked to 1).
- Coarse progress milestones (start / each tool name / done) are bridged up as the top-level agent's `AgentEvent`s; the sub-agent's own LLM token stream is not forwarded, to avoid two streams flooding the UI.
