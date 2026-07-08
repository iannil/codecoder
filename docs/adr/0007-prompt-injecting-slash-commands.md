# Prompt-injecting slash commands

Some slash commands are not handled purely locally: they construct an expanded prompt and forward it to the LLM via `AgentCommand::ProcessMessage`. `/grill-me` is the first instance. The visible TUI message shows the raw `/cmd args` the user typed; the LLM sees the *expanded* prompt, and only for that turn.

## Why this preserves the typo-safety invariant

[[0002-slash-command-local-dispatch]] guarantees mistyped commands never leak to the model. That still holds here because it is the **dispatcher's own expansion** — deterministic, code-authored — that reaches the LLM, not the user-typed text. An unknown `/cmd` still fails locally as a `System` error and is never sent.

## Consequences

- The expansion is ephemeral (this turn only); it is not persisted as the user's message content — the raw `/cmd args` is what the Session records.
- This is the mechanism by which local UX affordances (like a guided grilling flow) can inject rich structured prompts without the user typing them.
