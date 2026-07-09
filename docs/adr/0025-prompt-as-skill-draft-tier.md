# Prompt as the draft tier of the Skill kind

**Status**: Accepted; **implemented**. `Registry` scans `prompts/` with a `[draft]` marker, `use_skill` falls back `skills/` → `prompts/`, and `promote_prompt` (the 25th tool) atomically promotes a draft into a Skill. Tests cover the generate → use → promote lifecycle, missing-draft, and name-collision paths.

`generate_prompt` authors a `.md` into `prompts/`. Before this ADR that file was a **write-only orphan** — nothing scanned, injected, or executed it, and the fourth artifact silently contradicted the load-bearing Tool/Skill/Capability triple ([[CONTEXT.md]]). We resolve it not by deleting the tool but by naming what it is: a **Prompt is the draft / probationary tier of the Skill _kind_**, not a fourth kind. Same nature as a Skill (self-authored procedural knowledge, injected into context, executes nothing), lower maturity and lower priority. The triple stands *by kind*; Prompt is a maturity stage *within* the "learned idea" kind.

## Decisions

- **Registry scans `prompts/`** alongside `skills/` and `capabilities/`, folding drafts into the **same resident catalog** with a `[draft]` marker. No separate index.
- **No new activation tool.** Prompts are activated through the existing `use_skill` tool, which resolves by path (`prompts/<name>.md` vs `skills/<name>.md`). "Lower priority" is a **catalog presentation + selection-bias** concern only: drafts sort after Skills and carry `[draft]`, nudging the agent toward matured Skills. It is *not* an injection-order, name-override, or compaction rule.
- **Promotion is a new built-in tool `promote_prompt`** (the 25th tool) — the only atomic path that both writes `skills/<name>.md` and deletes `prompts/<name>.md` in one step (the agent has no `delete_file` tool, so it cannot do this by hand). Signature `promote_prompt { name, content? }`: omit `content` to promote the draft **verbatim**; pass `content` to land a **refined** final version, so the maturity step can carry real content change, not just a move.
- **Agent-triggered.** The agent decides a draft has proven itself across turns and promotes it. Permission is `Ask` at `write_file` level (same as `generate_skill`; writing into `skills/` executes nothing). Takes effect on the next visible `/reload`, consistent with all self-authoring.
- **Name collision errors.** If `skills/<name>.md` already exists, `promote_prompt` refuses rather than silently overwriting a matured, validated Skill.

## Considered options

- **Delete `generate_prompt`** (honor the triple by subtraction). Rejected: the agent wants a cheap place to park a half-formed heuristic before committing it to a durable Skill; a maturity ladder is worth more than a clean subtraction.
- **Promote `Prompt` to a true fourth kind** (its own glossary peer, its own `use_prompt`, its own priority mechanics). Rejected: it would dilute the "exactly three kinds" story for no semantic gain — a draft heuristic *is* procedural knowledge, i.e. the Skill kind.
- **Verbatim-only promotion (a pure `mv`).** Rejected: it would collapse the Prompt/Skill distinction to directory location alone. Allowing optional refinement on promote keeps "draft vs matured" a genuine content distinction.

## Consequences

- Built-in tool count: **24 → 25** (`promote_prompt` added). `generate_prompt` was always counted; it is no longer an orphan.
- Registry now scans **three** directories, not two — update `registry.rs` and every doc that says "scans skills/ and capabilities/".
- Docs to reconcile: `CONTEXT.md` (Prompt term added; Skill term now references it), `ARCHITECTURE.md` (module map, self-evolution loop, tool table), `README.md`, `CLAUDE.md` (the enumerated tool list — which already omitted `generate_prompt`, `review`, `confirm` — must be corrected to the real 25).
- The OpenAI-facing tool list grows by one (`promote_prompt`); the catalog absorbs drafts. Consistent with [[0020-skills-and-capabilities-registry]]: fixed tool list, growth in the catalog — with the single exception that promotion needed a real tool because it crosses directories atomically.
