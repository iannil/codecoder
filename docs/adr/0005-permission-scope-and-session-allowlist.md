# Permission scope and session allowlist

A permission grant has a durability (`PermScope`: `Once` | `AlwaysThisSession` | `AlwaysThisProject`) and applies to a `PermissionKey` (see [[0018-tool-trait-and-permission-keys]]), not a bare tool name. `AlwaysThisSession` grants live in an in-memory `Session Allowlist` (`HashSet<PermissionKey>`) cleared on process exit; `AlwaysThisProject` grants persist to `codecoder.json`.

## Consequences

- A call whose `PermissionKey` is already in the Session Allowlist (or the persisted project allowlist) skips the prompt entirely.
- **Ceiling rule**: invoking a `Shell`-environment Capability is capped at `AlwaysThisSession` and may never reach `AlwaysThisProject`, because a host-shell capability is the one self-modification escape hatch (see [[0022-self-authoring-safety-loop]]).
- The two allowlists are keyed identically (`PermissionKey`) but differ in lifetime and storage — the in-memory session set vs the on-disk project set.
