# CodeCoder

自主 AI agent 系统 — 事件驱动，文件系统即自我。本文件是术语表，定义项目中那些「不读代码就会理解错」的关键词。**只列项目特有概念**，通用编程术语（timeout、callback、iterator 等）不在此列。

## Interaction & UI

> **NOTE:** The following terms (Mode, Dialog, Popup, Overlay, Reasoning, Frame, DisplayState) are **legacy TUI concepts** from the pre-client-server architecture. Since ADR 0032 (client-server migration), the UI is handled by the `ccli` CLI client over stdin/stdout; permission/ask/confirm/plan/trust dialogs are rendered inline as `y/n` prompts. These terms are kept for historical reference and ADR consistency but no longer represent the current UI implementation.

**Mode**:
The TUI's current interaction context, governing how key presses are interpreted. Concrete modes: `INSERT` (normal input), `SEARCH` (Ctrl+F), `R-SEARCH` (Ctrl+R reverse search), `DIALOG` (permission/plan/ask overlay open), `HELP`, `MODEL` (model picker), `SLASH` (an input-completion popup is open — either the slash-command list or the `@`-file-completion list; both are non-blocking popups above the input), `BROWSE` (message-list browse after Up/Down on empty input). **Derived, not stored:** `TuiApp` holds no `mode` field — the active mode is *computed each frame* from authoritative sub-state via a fixed precedence chain (dialog → popup → search → browse → INSERT), so "exactly one mode per frame" is structurally true and the mode can never desync from what is actually open. Computing it during render keeps `Frame` read-only. Shown in the status bar. See [[0001-tui-keybinding-and-mode-semantics]].
_Avoid_: state, screen, view, panel.

**Dialog**:
A modal overlay that blocks the underlying UI and demands user input before any other action. Constructed as the `Dialog` enum (ToolPermission / PlanApproval / AskQuestion / Confirm). Only one dialog can be active at a time. While a dialog is open, the input box is in `DIALOG` mode.
_Avoid_: popup (use "popup" only for the non-blocking slash/file-completion lists), modal, window.

**Popup**:
A non-modal overlay that appears above the input area but does not block the rest of the UI — slash-completion list, file-completion list, model picker. Multiple popups cannot be active simultaneously, but a popup can be dismissed without effect (unlike a dialog).
_Avoid_: dialog, menu, dropdown.

**Overlay**:
Generic term covering both Dialog and Popup — anything rendered above the standard 3-zone layout (messages / input / status). When the docs say "Esc closes overlays", it means dialogs and popups alike.
_Avoid_: window, layer.

**Reasoning**:
A collapsible message variant (`MessageItem::Reasoning`) holding the LLM's chain-of-thought text. Rendered dimmed and collapsed by default; expanded via `Tab` ([[0001-tui-keybinding-and-mode-semantics]]). Distinct from `Assistant` (the final answer) and `System` (UI chrome).
_Avoid_: thinking, CoT, explanation, rationale.

**Frame**:
One render pass of the TUI. **Event/animation-driven, not a constant spin**: the main loop blocks on a unified channel and renders when a terminal input, an `AgentEvent`, or an animation `Tick` arrives — when fully idle it does not redraw (zero CPU). `frame_count: u64` is the monotonic counter incremented per actual render, used for animations (spinner, cursor blink); the animation `Tick` source only emits while something is animating (e.g. the agent is working). A frame reads the current `TuiApp` state and produces one terminal draw; it must not mutate app state. See [[0024-tui-viewport-and-render-loop]].
_Avoid_: tick (a Tick is the animation-clock message that may *trigger* a frame; the frame is the render), refresh, repaint, iteration.

**DisplayState**:
The per-message UI presentation state held in `TuiApp`'s `HashMap<MessageId, DisplayState>` — chiefly whether each collapsible block (a `Reasoning` item, or a `ToolResult` longer than the fold threshold) is folded or expanded. Keyed by `MessageId` so `/clear`, eviction, and compaction never mis-anchor it (see [[0015-unified-message-model]]). Purely a view concern: it never travels to the provider and is not persisted in the Session. Distinct from `Mode` (a whole-TUI context) — DisplayState is scoped to one message.
_Avoid_: state (unqualified), view, fold state (fold is one field of it), render state.

## Permissions

**Permission Scope**:
The durability of a permission grant, expressed as the `PermScope` enum: `Once` (re-prompts next time), `AlwaysThisSession` (no more prompts this session), `AlwaysThisProject` (persisted to codecoder.json). A grant applies to a `PermissionKey`, not a bare tool name. **Ceiling rule:** invoking a `Shell`-environment Capability (`run_capability:<name>@shell`) is capped at `AlwaysThisSession` — it may never be granted `AlwaysThisProject`, because a host-shell Capability is the one self-modification escape hatch ([[0022-self-authoring-safety-loop]]). See [[0005-permission-scope-and-session-allowlist]].
_Avoid_: permission level, duration, persistence mode.

**Permission Key**:
The fine-grained string a tool derives from its call args to decide *what* a permission grant covers, returned as `Permission::Ask { key }` from `Tool::permission`. A read-only tool returns `Permission::None` (never prompts). Granularity is the tool's choice and lands at the **command-class / path-prefix** sweet spot — `run_command` yields e.g. `run_command:git` (allowing `git status` does not allow `git push`), *not* the whole `run_command` (too coarse — one grant would free every shell command) and *not* the exact argv (too fine — `AlwaysThisSession` would never hit). See [[0018-tool-trait-and-permission-keys]].
_Avoid_: permission id, rule, pattern, scope (scope is the durability, this is the target).

**Session Allowlist**:
The in-memory `HashSet<PermissionKey>` kept by the agent thread for the current session. A call whose `PermissionKey` is in this set skips the permission prompt entirely. Cleared when the process exits. Distinct from the project-scope allowlist persisted in codecoder.json (also keyed by `PermissionKey`).
_Avoid_: whitelist, allowed tools, permission cache.

## Persistence

**Session**:
A saved conversation: a JSON file under `sessions/` containing `messages`, `model`, `token_count`, metadata, and `schema_version`. **Autosaved on every message append** (full-file rewrite) so any kill/power-loss resumes to the last complete message. Loaded via `/resume` through a **forward-migration chain** keyed on `schema_version` (`migrate_vN_to_vN+1`); a load that fails to migrate **errors and preserves the original file**, never silently overwriting. `Reasoning` items are **persisted but not replayed** to the provider on resume. See [[0004-session-persistence-and-migration]].
_Avoid_: conversation, chat, history (history is the in-memory input buffer for Up/Down navigation — see below).

**Memory**:
The persistent key-value store under `memory/`, surviving across sessions. Beyond small facts, it doubles as the agent's **discoverable index / ledger** of locally-stored data: when a Capability fetches data from the internet, the **bytes** land as a file (conventionally under `data/`) while a Memory entry records the pointer + provenance (`data:<name> → { path, source, fetched, desc }`), so the agent can later discover what data it holds and autonomously choose whether to use it. Freshness/re-fetch is the fetching Capability's concern, not a storage-layer TTL. No first-class `Dataset`/`Artifact` concept exists — data is just files plus a Memory index. Distinct from Session (per-conversation) and History (in-memory input buffer). Listed via `/memory`.
_Avoid_: cache, store, dataset, artifact, knowledge base, database.

**Context Working Set**:
The in-memory, **derived** subset of a Session's messages that the `Provider` actually packs into a request — never stored as the source of truth. It is recomputed from the full-fidelity persisted `messages` against the current model's context window, so switching to a larger-window model automatically "decompresses" an old conversation. Compare [[0001-tui-keybinding-and-mode-semantics]]'s derived `Mode`: anything derivable is derived, not stored as state that could drift.
_Avoid_: context, window, history, buffer, working memory.

**Compaction**:
The act of shaping the Context Working Set when `token_count` approaches the model window (~75%). **Tiered and hybrid**: first drop the cheapest, bulkiest items (old `ToolResult` bodies and `Reasoning`, which aren't replayed anyway), then summarize the oldest remaining dialogue span into a synthetic `System` summary; the first user goal is an anchor that is never evicted. **Invariant: Compaction never destroys the persisted record** — the on-disk Session keeps every message; a summary lives as a `compaction` side-field/overlay, not a replacement for `messages`. See [[0023-context-compaction]]. **v1 status: tier 1 live (drop `Reasoning` + elide old `ToolResult` bodies, anchoring the first goal and a recent tail); tier 2 (LLM summarization of the oldest span) still deferred.**
_Avoid_: truncation, eviction (eviction is only one tier of it), summarization (only the second tier), pruning, trimming.

**History** (input history):
The in-memory `Vec<String>` of previously submitted user inputs, navigated via `Ctrl+Up`/`Ctrl+Down`. Not persisted. Distinct from Session.
_Avoid_: log, recents, message history.

**MessageId**:
A per-session monotonic `u64` assigned to each `Message` when appended to history and persisted with it. Anchors UI display state in `TuiApp`'s `HashMap<MessageId, DisplayState>` so eviction/`/clear` don't mis-anchor. **Distinct from `ToolCall.id`**: `MessageId` is the UI/persistence identity of a whole message; `ToolCall.id` is a **provider-neutral** correlation id linking a `ToolCall` item to its `ToolResult` item. At the API boundary the `Provider` trait maps it to the wire format — for the canonical OpenAI protocol, to `tool_calls[].id` on the assistant turn and `tool_call_id` on the `role:"tool"` turn. See [[0015-unified-message-model]] and [[0017-provider-neutral-message-model]].
_Avoid_: index, message index, uuid (it is not a UUID; it is a session-local counter).

**Work Graph**:
The durable, dependency-ordered graph of **Milestone** nodes, persisted to `workgraph.json` with the SAME versioned / atomic-write / forward-migration discipline as Session (`src/workgraph.rs`). It is the "事前构造之图" (what the agent intends to do) opposite the Session's "事后记录树" (what happened) — the two halves of the agent's file-backed work state. Survives context resets and Compaction (it is a file, not conversation). Managed by the `milestone` tool; consulted via `next_ready()` (the lowest-id `Pending` node whose deps are all `Done`). Supersedes the old flat `todo`; a legacy `todos.json` is migrated forward on first read. See `docs/design/2026-07-19-plan-work-graph.md` and `docs/audit/0002-first-class-citizen-analysis-2026-07-19.md` (#2).
_Avoid_: todo list, backlog, plan (plan is the one-shot approval gesture), roadmap.

**Milestone**:
One node of the Work Graph: `id · title · deps · status`. `status` is `pending / in_progress / blocked / done / hypothesis / locked` — where **`blocked` is DERIVED** from unmet dependencies (recomputed, never the authoritative record of intent), the others set explicitly by an action. `hypothesis` and `locked` are diagnostic-side variants (P2, for future inference-tree use in the workgraph context). Per-milestone gates (`acceptance`/`command`/review/`needs_fix`) were removed: verification is the agent's own responsibility, and completion is self-reported by the agent.
_Avoid_: task (that word is reserved — see Background Agent / sub-agent), step, ticket, issue.

## Extensibility & Self-Evolution

These three are the load-bearing distinction of the whole system: **Tool is innate, Skill is a learned idea, Capability is a grown limb.** A Skill only changes *how the agent thinks*; a Capability grows *a new hand that acts*. Confusing them is the single most damaging naming error in the codebase.

**Tool**:
A primitive compiled into the binary and shipped with the release (40 of them: `read_file`, `run_command`, `agent`, `reason`, `generate_skill`, `generate_capability`, `promote_prompt`, `lsp`, `mcp_call_tool`, `task_create`, `cron_create`, `send_message`, …). The uniform thing an LLM invokes as a `tool_call` within a turn (see `Tool` trait, [[0018-tool-trait-and-permission-keys]]). Fixed at build time — a runtime-authored executable is **not** a Tool, it is a Capability.
_Avoid_: capability, skill, function, command.

**Skill**:
A self-authorable `.md` document under `skills/`, auto-registered, packaging **procedural knowledge** (how to approach a class of task). When selected it is **injected into context** and changes how the agent reasons; it executes nothing by itself. Authored at runtime via `generate_skill`. The **matured** form of a [[prompt]]: a Prompt that has proven useful is promoted into a Skill. See [[0020-skills-and-capabilities-registry]].
_Avoid_: capability, tool, prompt, plugin.

**Prompt**:
A self-authored `.md` draft under `prompts/`, authored at runtime via `generate_prompt`. **Not a fourth kind** — it is the **draft / probationary tier of the [[skill]] kind** ("learned idea"): same nature as a Skill (injected procedural knowledge that changes *how the agent thinks*, executes nothing), but lower maturity and **lower priority** than a Skill. It is where the agent parks a half-formed heuristic before it has earned promotion to a durable Skill — promotion is the `promote_prompt` tool, which atomically writes `skills/<name>.md` and deletes the draft (erroring if a Skill of that name already exists). The Tool/Skill/Capability triple remains the load-bearing distinction *by kind*; Prompt is a maturity stage *within* the Skill kind, not a peer of it. See [[0025-prompt-as-skill-draft-tier]].
_Avoid_: skill (a Prompt is the pre-promotion draft, a Skill is the matured form), capability, tool, prompt-injecting slash command (unrelated — that is a TUI dispatch concept).

**Capability**:
A self-authored **executable artifact** the agent builds to gain a genuinely new runtime action beyond the built-in Tools — a one-shot script, a compiled program, an API client, or a long-running service. Runs in an **Environment** (shell / wasm / docker) under a **Lifecycle** (one-shot / on-demand / persistent). Authored at runtime via `generate_capability`. The forms "tool / api / service" from the product vision are **lifecycle shapes of a Capability, not separate concepts**. See [[0020-skills-and-capabilities-registry]] and [[0021-capability-environments-and-lifecycle]].
_Avoid_: tool (only built-ins are Tools), skill (only non-executing knowledge is a Skill), plugin, module, service (a service is one lifecycle shape of a Capability, not a synonym).

**Registry**:
The startup-and-`/reload`-time scanner that indexes `skills/`, `prompts/`, and `capabilities/` into a compact **catalog** (per entry: name + one-line description) kept resident in context, so the agent knows what exists and can autonomously decide whether to use it. `prompts/` entries (draft-tier Skills, [[0025-prompt-as-skill-draft-tier]]) are catalogued with a `[draft]` marker and sorted after matured Skills. Full skill text is injected only on activation (built-in `use_skill` tool, which resolves a name against `skills/` first, then `prompts/`); a Capability is executed only when invoked (built-in `run_capability { name, args }` dispatcher, [[0020-skills-and-capabilities-registry]]). Distinct from the OpenAI-facing tool list, which is fixed at the built-in Tools. The **catalog** (lightweight, always resident) is distinct from the **registry** (the scanner/index that produces it).
_Avoid_: index, store, loader, registry (for the catalog), catalog (for the scanner) — keep the two named parts distinct.

**Environment**:
Where a Capability executes, the `Environment` enum: `Shell` (host process — trusted domain, still permission-gated per call), `Wasm` (wasmtime + WASI isolation — no network, restricted FS), `Docker` (container isolation for any language — no network, read-only workspace mount, CPU/memory limits). Each Capability **declares** its `environment` in its manifest (language may suggest a default, but the declaration wins). Supersedes the old L0/L1/L2 tiering; the former "L0" is dropped (it executed no code, so it was never an environment). See [[0021-capability-environments-and-lifecycle]].
_Avoid_: sandbox (Shell is not sandboxed; use "environment" for the general term), runtime, tier, level, L0/L1/L2.

**Lifecycle**:
How long a Capability's execution lives, the `Lifecycle` enum: `OneShot` (run once, capture stdout, destroy), `OnDemand` (started on invocation, briefly reusable, then reclaimed), `Persistent` (a long-running background service surviving across turns, invoked repeatedly over network/IPC). A `Persistent` capability that crashes is **not auto-restarted** — it is marked `Failed` and left visible in the catalog for the agent to decide, and all `Persistent` services are bound to the CodeCoder process lifetime (dropped/killed on exit, never surviving a restart). See [[0021-capability-environments-and-lifecycle]].
_Avoid_: mode, duration, kind, type.

**Running Service Table**:
The in-memory map (name → PID / listening address / health) of currently-live `Persistent` capabilities. A `Persistent` capability registers its port/socket here on start; later invocations reach it via that address rather than re-spawning. Distinct from the Registry (which catalogs *authored* skills/capabilities on disk) — this tracks *running* ones. Cleared on process exit.
_Avoid_: registry, process pool, service registry, daemon table.

## LLM Protocol

**Provider**:
The trait abstracting one LLM backend, responsible for translating the provider-neutral message model to/from a concrete wire protocol and streaming responses back as `AgentEvent`s. Concrete impls: `OpenAiClient` (the canonical chat-completions protocol) and `StubClient` (deterministic fake used when `CODECODER_API_KEY` is unset). Adding Anthropic later is a new `Provider` impl, not a change to the message model. See [[0017-provider-neutral-message-model]].
_Avoid_: client (unqualified — `OpenAiClient` is *a* provider), backend, adapter, driver, LLM.

**MessageItem**:
One content element inside a `Message`'s `items: Vec<MessageItem>`. Variants: `Text` (final assistant/user prose), `Reasoning` (see above), `ToolCall { id, name, args }`, `ToolResult { call_id, .. }`. Provider-neutral: the same item set serializes identically to a session regardless of which `Provider` produced it. A single assistant turn may carry several `ToolCall` items (parallel tool use).
_Avoid_: block, part, chunk, content, segment.

## Agents

**Sub-agent**:
A child `AgentLoop` spawned by the `agent` **or** `review` tool to run a delegated task on the parent's behalf, running on its own thread that the parent's tool call joins. (`review` is **not** a distinct concept — it is a convenience wrapper that pre-seeds the task prompt with the architecture-drift **rubric** and dispatches through the *identical* sub-agent machinery: same `Toolbox::read_only_child()` set, same depth-lock of 1, same no-user-channel contract; on return it parses the child's prose into a structured **Review Verdict** (see below). There is no separate "review agent" type.) Reports its result **back to the parent agent, never directly to the user** — only the top-level agent owns the user-facing channel. **Read-only by contract has a precise, enforced meaning: its tool set is a curated subset of the tools returning `Permission::None`** — the 9 tools in `Toolbox::read_only_child()` (`read_file`, `list_directory`, `use_skill`, `glob`, `grep`, `search_web`, `search_github`, `reverse_api`, `diff`) and no `ask_user`. (The local-scratch `Permission::None` tools `plan`/`milestone`/`memory` are deliberately excluded — read-only means no side effects, not merely no prompt.) This falls out of it having no user channel: a permission prompt would have no one to answer it, so permission-requiring tools are simply not in its set (no bubbling). It **cannot spawn further sub-agents** (depth locked to 1). Coarse progress milestones (start / each tool name / done) are bridged up as the top-level agent's `AgentEvent`s; the sub-agent's own LLM token stream is not forwarded. Distinct from the top-level agent and from a Background Agent (see below). See [[0019-sub-agent-capability-boundary]].
_Avoid_: worker, thread, task, child process.

**Review Verdict**:
The structured outcome of the `review` tool: one of `pass` / `needs_fix` / `rebuild`, plus the four **Drift Signals**. The sub-agent self-reports a verdict, but the kernel is the authority: it takes the **more-severe** of the reported verdict and the verdict *derived* from the signals (a lenient reviewer can never downgrade below what the signals imply; a foundation fail forces `rebuild`). If the reviewer ignores the two-line output contract, the verdict defaults to `needs_fix (unparsed)` — never a silent pass. Parsed and aggregated in `src/review.rs` (pure functions); the `review` interception in `agent.rs` formats it as a deterministic header above the sub-agent's prose. The `review` tool remains a standalone architecture-review aid; it is no longer wired into per-milestone acceptance (which is now agent self-reported).
_Avoid_: grade, score, rating, result (unqualified).

**Drift Signal**:
One of the four architecture-drift axes a Review Verdict reports, each `ok` / `warn` / `fail`: **foundation** (silently altering solidified ground — public signatures, message model, permission keys, session format, ADR-fixed contracts; a red line), **over_engineering** (needless deps/abstractions), **volume** (files/functions bloating, duplication), **terminology** (new names colliding with the `_Avoid_` glossary entries here). Ported from the engineer-inspector skill, calibrated to codecoder. Only a `foundation` fail escalates to `rebuild`; any other fail is `needs_fix`.
_Avoid_: check, lint, rule, metric.

**Background Agent**:
A full agent (its own LLM loop) that runs **autonomously on a schedule with no user present**. Two axes separate the three "runs-in-the-background" concepts: **has an LLM loop?** and **is a user present?** A Background Agent has a loop and no user present; a **Sub-agent** has a loop but a user present (synchronously awaited, read-only); a **Persistent Capability** has *no* LLM loop (it is a long-running service — a limb, not a thinker). **v1 ships a headless one-shot runner** (`CODECODER_BG_TASK=<task>`, see [[0026-background-agent-headless-runner]]): a full-loop agent runs one task with no user present, using pre-authorized `codecoder.json` permissions (any un-authorized Ask-tool is auto-denied, never prompted). When `CODECODER_BG_TASK` is unset, the runner falls back to the Work Graph's next ready milestone and auto-advances through up to 3 milestones. Scheduling is external; SIGINT/scheduler/multi-runner limits remain deferred. See [[0019-sub-agent-capability-boundary]].

**Inference Tree** (Causal Tree):
A persistent, dependency-ordered tree of causal-reasoning nodes for root-cause analysis, stored in `causal_tree.json` (independent from `session.json` and `workgraph.json`). Each node has a `question`, `status` (hypothesis / locked), and optional `margin` / `leverage` / `terminal` metadata. Managed by the `reason` tool (actions: add / status / margin / list / trace). The methodology for using it is in `skills/debug-causal.md`. Forms the "事后诊断之树" half of the diagnostic→construction closed loop, where inference-tree findings (high-margin, high-leverage nodes) are converted into Work Graph milestones via `milestone add`. See `docs/superpowers/specs/2026-07-20-inference-tree-spec.md`.
_Avoid_: debug tree, trace tree, root-cause tree (use "inference tree" or "causal tree"), reason tree.

## Code Conventions

**Slash Command**:
An input beginning with `/` that is intercepted by the local dispatcher in TUI mode and never forwarded to the LLM. See [[0002-slash-command-local-dispatch]]. Unknown commands produce a System error.
_Avoid_: command (too generic — use "shell command" or "agent command" for other meanings), macro.

**Prompt-Injecting Slash Command**:
A slash command that constructs an expanded prompt and forwards it via `AgentCommand::ProcessMessage`. ADR 0002's typo-safety invariant is preserved because the dispatcher's own expansion (not user-typed) is what reaches the LLM. The visible TUI message shows the raw `/cmd args` the user typed; the LLM sees the expanded prompt for that turn only. `/grill-me` is the first instance. See [[0007-prompt-injecting-slash-commands]].
_Avoid_: prompt command (ambiguous — could mean "command for prompts"), macro, template.

**Agent Command**:
A message sent from the TUI thread to the agent thread via the `cmd_tx` channel, typed as the `AgentCommand` enum (ProcessMessage, Shutdown, Cancel, etc.) — carrying only **user-initiated** intents. Distinct from slash commands (which are user-typed) and shell commands (which agent tools execute). Note: a permission/ask reply is **not** an Agent Command — it travels back over the `reply_tx` oneshot carried by the originating `AgentEvent`, never over `cmd_tx`, so a pending request's answer can never be reordered behind a new `ProcessMessage`. See [[0016-channel-topology-and-event-model]].
_Avoid_: command (unqualified), request, message.

**Agent Event**:
A message sent from the agent thread back to the TUI thread over the `event_rx` channel, typed as the `AgentEvent` enum. Carries two rhythms of one-way traffic — high-frequency LLM stream deltas and low-frequency structured state changes (tool start/end, reasoning, sub-agent progress). Blocking round-trips (permission, `ask_user`) are a third variant that embeds a `reply_tx` oneshot the TUI answers directly; the agent thread blocks on `reply_tx` until then. Only the top-level agent owns an `AgentEvent` channel to the user; sub-agents report to their parent. See [[0016-channel-topology-and-event-model]].
_Avoid_: message, output, signal, notification.

**Theme**:
A struct (`Theme`) holding all color definitions used by the TUI, held by `TuiApp` and read by every render function. Swappable between `dark()` and `light()` constructors. See [[0003-central-theme-struct]].
_Avoid_: color scheme, palette, skin, style (style refers to a single `ratatui::style::Style` instance, not the global theme).
