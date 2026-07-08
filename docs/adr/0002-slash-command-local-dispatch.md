# Slash commands are dispatched locally, never sent to the LLM

An input beginning with `/` is intercepted by a local dispatcher in the TUI and handled in-process — it is **never forwarded to the LLM**. An unrecognized slash command produces a `System` error message rather than being sent as a prompt.

## Typo-safety invariant

The value of local dispatch is that a mistyped command (`/exitt`, `/hlep`) can never leak to the model as a chat turn, waste tokens, or produce a confusing assistant reply. This invariant is load-bearing and is relied on by [[0007-prompt-injecting-slash-commands]], where the *dispatcher's own* expansion — not user-typed text — is what reaches the LLM.
