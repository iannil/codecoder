# TUI viewport and render loop

The TUI uses a **fullscreen managed viewport** (alternate screen), and its render loop is **event/animation-driven** over a unified channel — not an inline viewport, and not a constant 60fps spin.

## Fullscreen viewport, not inline

Two ratatui models were on the table: `Viewport::Inline` (commit finished messages to the terminal's native scrollback via `insert_before`, shell-like, preserves native copy/scroll) vs `Viewport::Fullscreen` (alternate screen, we own and can re-render every row each frame).

We chose **fullscreen**, because the semantics already committed in CONTEXT.md require re-rendering arbitrary past rows, which inline mode cannot do once a line has scrolled into native scrollback:

- **Reasoning** folds/expands after the fact (`Tab`).
- **Dialog** is a blocking modal drawn *over* the transcript.
- **BROWSE** navigates a selection highlight through past collapsible blocks.

The cost is real — we give up native terminal scrollback and native text selection — but that is the standard trade for a rich TUI (vim/htop/lazygit); mouse-wheel scroll and a copy-mode can be added later. claude-code gets a shell-like feel from Ink's `<Static>`, but switches to an AlternateScreen for its rich-interaction surfaces — the same split, resolved here toward the rich surface.

## Event/animation-driven render loop

`crossterm::event::poll` can only wait on terminal input, not our `AgentEvent` channel. Rather than busy-poll with a short timeout (which burns CPU while idle and still only samples the agent channel every ~33ms), an input-reader thread converts crossterm events into messages on a **unified channel** that also carries `AgentEvent`s. The main loop **blocks on `recv()`** — zero CPU when idle, and streaming tokens are handled the instant they arrive. Animation (spinner, cursor blink) is driven by a `Tick` source that emits **only while something is animating** (e.g. the agent is working, ~20fps); when fully idle, no ticks, no redraws. `frame_count` counts actual renders. This refines the `Frame` definition away from "constant ~60fps."

## Newline vs submit: Shift+Enter via keyboard enhancement, Ctrl+J fallback

`Enter` submits; `Shift+Enter` inserts a newline. Terminals cannot distinguish `Shift+Enter` from `Enter` by default, so the TUI enables the **Kitty keyboard protocol** (`crossterm` `PushKeyboardEnhancementFlags`). On supporting terminals (kitty, ghostty, foot, WezTerm, recent iTerm2) `Shift+Enter` is detected natively; on terminals without it, `Ctrl+J` is the universal newline fallback so multiline input always works. Bracketed paste is enabled so pasted multi-line text lands intact.
