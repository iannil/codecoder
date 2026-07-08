# Capability environments and lifecycle

A Capability declares two orthogonal properties in its manifest: an `Environment` (where it runs) and a `Lifecycle` (how long its execution lives). This replaces the old L0/L1/L2 sandbox tiering, which conflated isolation strength with a non-executing "L0" that was never an environment at all.

## Environment

`enum Environment { Shell, Wasm, Docker }`

- `Shell` — host process; trusted domain but permission-gated per call.
- `Wasm` — wasmtime + WASI isolation (no network, restricted FS). Uses wasmtime directly, **not** lunatic, and v1 accepts only `.wasm`/`.wat` input — compiling source to wasm is deferred as its own project.
- `Docker` — container isolation for any language (no network, read-only workspace mount, CPU/memory limits). If the Docker daemon is absent, `run_capability` **errors explicitly** rather than silently downgrading to host execution — a silent downgrade would make the word "sandbox" lie.

The Capability declares its `environment`; language may suggest a default but the declaration wins.

## Lifecycle

`enum Lifecycle { OneShot, OnDemand, Persistent }`

- `OneShot` — run once, capture stdout, destroy.
- `OnDemand` — started on invocation, briefly reusable, then reclaimed.
- `Persistent` — a long-running background service surviving across turns, invoked over network/IPC.

## Persistent service policy

- **No auto-restart.** A crashed `Persistent` capability is marked `Failed` and left visible in the catalog for the agent to decide — a silent restart would mask bugs.
- **Bound to process lifetime.** All `Persistent` services are dropped/killed on CodeCoder exit; they never survive a restart (avoids orphaned services and leaked ports). A true daemon is a separate future project.
- **Addressed, not re-spawned.** A `Persistent` capability registers its port/socket in the in-memory Running Service Table on start; later invocations reach it there.
