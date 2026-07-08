# Session persistence and migration

Sessions are JSON files under `sessions/`, **autosaved on every message append** via full-file rewrite (sessions are small; the guarantee that any process kill resumes to the last complete message is worth the write). Schema evolution is handled by a **versioned forward-migration chain**: each `schema_version` has a `migrate_vN_to_vN+1(json) -> json`, and `/resume` runs a file from its stored version up to the current one.

## Why a migration chain, not lenient deserialization

`schema_version` only earns its place if it drives real migrations. Relying on `#[serde(default)]` tolerance instead would silently mistranslate whenever a field's *semantics* change (not just its presence), corrupting old conversations invisibly. A load that cannot be migrated **errors and preserves the original file** — it never silently overwrites a session it failed to read.

## Reasoning: persisted, not replayed

`Reasoning` (chain-of-thought) items are written to the session so the UI can still expand historical reasoning after `/resume` and so it is auditable. But the `Provider` translation layer **skips `Reasoning` items when building a request** — replaying CoT as ordinary assistant content is wrong under provider semantics and wastes tokens. So: reasoning is stored but not fed back.
