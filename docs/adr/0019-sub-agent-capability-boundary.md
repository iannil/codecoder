# Sub-agent capability boundary = the Permission::None tool set

A sub-agent's "read-only by contract" is given a precise, enforced meaning: **its tool set is exactly the tools returning `Permission::None`** (`read_file`, `glob`, `grep`, `list_directory`, `search_web`, `search_github`, `reverse_api`), plus no `ask_user`. This falls out of a structural fact rather than a policy choice.

## Why it's forced, not chosen

Only the top-level agent owns the user-facing channel (see [[0016-channel-topology-and-event-model]]). A sub-agent therefore has no one who could answer a permission prompt. Rather than bubble prompts up to the parent — which would make a user field permission dialogs for work they didn't directly order, blurring accountability — permission-requiring tools are simply **not in the sub-agent's set**. The permission question is eliminated because the sub-agent cannot reach any tool that would raise it.

## Consequences

- A sub-agent runs on its own thread that the parent's `agent`-tool call joins; it reports its result back to the parent as that tool's `ToolOutput`, never to the user.
- It **cannot spawn further sub-agents** (depth locked to 1).
- Coarse progress milestones (start / each tool name / done) are bridged up as the top-level agent's `AgentEvent`s; the sub-agent's own LLM token stream is not forwarded, to avoid two streams flooding the UI.
