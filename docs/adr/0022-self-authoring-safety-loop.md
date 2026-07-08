# Self-authoring safety loop: gate execution, not authoring

In a system that writes its own Capabilities and can run them on the host shell, the permission gate lives at **execution**, not authoring. `generate_skill` / `generate_capability` merely write files into `skills/` / `capabilities/` and are gated at `write_file` level (cheap and safe). The real gate is `run_capability`, keyed by **capability name + environment** (e.g. `run_capability:foo@shell`).

## Recursion boundary

Under the dispatcher model (see [[0020-skills-and-capabilities-registry]]) a Capability is just an executable inside its Environment — it has no access to the agent's tool set or main loop, so it **cannot recursively author more capabilities**. Only the agent itself authors, via a deliberate `generate_capability` call. Self-modification is confined to that one visible path.

## The one escape hatch

A `Shell`-environment Capability has host filesystem write access and could bypass `generate_capability` by writing directly into `capabilities/`/`skills/`. `Wasm`/`Docker` cannot (isolated / read-only workspace). This residual risk is accepted but constrained:

- A `Shell` capability's writes into `capabilities/`/`skills/` take effect only at the next `/reload` — no hot registration, so self-modification must pass through one visible reload.
- Granting a `Shell` capability is **capped at `AlwaysThisSession`** — it may never reach `AlwaysThisProject` (see [[0018-tool-trait-and-permission-keys]] / Permission Scope). `Wasm`/`Docker` capabilities, being isolated, may be granted more freely.
