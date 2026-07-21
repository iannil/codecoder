# Project trust: a load-time gate for the disk "self"

"Filesystem as self" ([[0020-skills-and-capabilities-registry]]) means the agent's
identity and abilities come from disk: `AGENTS.md`, `CONTEXT.md`, `skills/`,
`prompts/`, `capabilities/`, and the `codecoder.json` execution allowlist
([[0005-permission-scope-and-session-allowlist]]). Cloning a repository and
launching codecoder inside it therefore lets **that repository** inject its
`AGENTS.md`/skill text into the agent's identity (prompt injection) and
pre-authorize dangerous tool calls via its `codecoder.json` (execution). The
permission gate ([[0018-tool-trait-and-permission-keys]]) governs *runtime
execution* but never asked whether the disk "self" should load at all.

This ADR adds a second, **orthogonal load-time gate**: trust. It implements
roadmap Wave 0 #5 ([[0027-pi-comparison-and-borrowing-roadmap]]), borrowed from
pi's `trust-manager` / `TRUST_REQUIRING_PROJECT_CONFIG_RESOURCES`.

## Two gates, not one

- **Permission** (runtime, ADR 0005/0018): may *this* tool call execute now?
- **Trust** (load-time, this ADR): may the project's disk "self" enter the agent
  at all?

Both are required. Native tools and the compiled-in base identity are never
gated — they do not come from disk, so no clone can weaponize them.

## The decision store

Decisions are stored **globally**, never in the project — a repository must not
vouch for itself. `~/.codecoder/trust.json` (override: `CODECODER_TRUST_FILE`)
maps canonical directory → `trusted`/`untrusted`, resolved **nearest-ancestor**:
a trusted parent trusts its children; a nearer explicit decision overrides an
ancestor's.

## Resolution at construction

`AgentLoop::build` resolves a `TrustState { Trusted, Untrusted, Pending }`:

1. A recorded decision always wins.
2. Undecided + **headless** (Background Agent, [[0026-background-agent-headless-runner]]):
   there is no one to prompt → `CODECODER_DEFAULT_TRUST` (`never` default →
   Untrusted; `always`/`once` → Trusted), never persisted.
3. Undecided + **sub-agent** ([[0019-sub-agent-capability-boundary]], no user
   channel): Untrusted (safe default).
4. Undecided + interactive + **no trust-requiring resources on disk**: Trusted —
   there is nothing to gate, so don't bother the user.
5. Undecided + interactive + resources present: **Pending**.

The disk "self" (system prompt from `AGENTS.md` + catalog; the `codecoder.json`
allowlist) loads only when `Trusted`. Untrusted/Pending run on the base identity
+ native tools with an empty allowlist.

## Resolving `Pending`: the first-turn prompt

An interactive `Pending` agent asks once, before its first turn, via a blocking
`AgentEvent::TrustPrompt { root, reply_tx }` answered by a TUI `Trust` dialog
(Dialog semantics per [[0016-channel-topology-and-event-model]]). The reply
(`TrustReply`):

- **Always** → record Trusted (persists), load self.
- **Once** → Trusted for this session only (no persist), load self.
- **Never** → record Untrusted (persists), stay empty.
- Dropped channel (no responder) → Untrusted, and don't ask again.

`/reload` re-scans the disk self only when Trusted; otherwise it reports that
nothing was reloaded.

## Interaction with Background pre-authorization (ADR 0026)

A Background Agent pre-authorizes tools via `codecoder.json`. Under trust, that
file is **ignored unless the project is trusted** — otherwise a cloned repo's
`codecoder.json` could pre-authorize `run_command:rm -rf` on a bare headless run.
A legitimate operator therefore opts in explicitly: `CODECODER_DEFAULT_TRUST=always`
or a recorded decision. This tightens ADR 0026's contract: pre-authorization now
additionally requires trust.

When a headless run denies an Ask-tool because the project is **not** trusted, the
denial message must name that root cause — "project not trusted; the codecoder.json
allowlist is NOT loaded" plus the `CODECODER_DEFAULT_TRUST` remediation — rather than
the bare "not in project allowlist" used when the allowlist *is* loaded but the key is
genuinely absent. A confused operator who did pre-authorize the key must be able to
diagnose that the file was never loaded. (Default trust itself is unchanged; this is a
diagnosability requirement, not a safety relaxation.)

## Trust-requiring resources

`AGENTS.md`, `CONTEXT.md`, `codecoder.json` (files) and `skills/`, `prompts/`,
`capabilities/` (dirs). Their presence is what turns an undecided interactive
project from auto-Trusted into Pending.

## Deferred (not in v1)

Per-artifact signing/verification (pi does none either), a trust-revocation UI,
and team/remote trust synchronization are out of scope.
