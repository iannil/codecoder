# Skills & Capabilities: registry, resident catalog, dispatcher invocation

Self-authored Skills and Capabilities are **discovered and selectively activated**, not always resident in the prompt. A `Registry` scans `skills/` and `capabilities/` at startup and on `/reload` into a compact **catalog** (name + one-line description) that stays in context, so the agent knows what exists and can autonomously choose whether to use each — without a hundred entries bloating every request.

## Activation

- **Skill** (pure knowledge): activated via a built-in `use_skill` tool that injects the full `.md` into subsequent turns. It executes nothing.
- **Capability** (executable): invoked via a built-in `run_capability { name, args }` dispatcher. The Registry locates it and executes it in its declared Environment/Lifecycle (see [[0021-capability-environments-and-lifecycle]]).

## Considered options

- **Dynamic tool registration** — each Capability exposed to the LLM as its own `tool_call`, matching "filesystem-as-self" most literally. Rejected for v1: it requires per-turn dynamic tool schemas and bloats the tool list as capabilities accumulate. Kept as a future evolution once capability counts and stability are proven.

## Consequences

- The OpenAI-facing tool list is **fixed** at the built-in Tools; growth happens in the catalog, not the tool list.
- Each catalog entry carries a `usage` blurb so the agent knows how to call `run_capability` without a formal per-capability schema.
