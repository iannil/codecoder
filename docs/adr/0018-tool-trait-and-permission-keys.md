# Tool trait and fine-grained permission keys

A `Tool` reports its own permission requirement from its call args, so permission grants are keyed by a fine-grained `PermissionKey` string rather than a bare tool name. This stops one grant on `run_command` from silently freeing every shell command for the session.

## Shape

```rust
trait Tool {
    fn name(&self) -> &str;
    fn schema(&self) -> serde_json::Value;              // params JSON schema, fed to the LLM
    fn permission(&self, args: &Value) -> Permission;   // tool self-reports side effect + key
    fn run(&self, args: Value, ctx: &mut ToolCtx) -> Result<ToolOutput>;
}

enum Permission {
    None,                    // read-only, never prompts (read_file, glob, grep, list_directory, ...)
    Ask { key: String },     // prompts; key decides allowlist-hit granularity
}
```

## Granularity is the sweet spot

The `PermissionKey` lands at the **command-class / path-prefix** level — `run_command` yields e.g. `run_command:git` (allowing `git status` does not allow `git push`). Not the whole tool name (too coarse — one grant frees everything) and not the exact argv (too fine — `AlwaysThisSession` would never hit and the user gets prompt-stormed).

## Consequences

- The `Session Allowlist` is `HashSet<PermissionKey>`, not `HashSet<tool_name>`.
- `Permission::None` does double duty: it is also the exact capability boundary of a sub-agent (see [[0019-sub-agent-capability-boundary]]).
