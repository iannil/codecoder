# Central Theme struct

All TUI colors live in a single `Theme` struct held by `TuiApp` and read by every render function, rather than being scattered as literals across the render code. It is swappable via `dark()` / `light()` constructors.

## Why

Centralizing colors makes theming a one-line swap and keeps render functions free of hardcoded color decisions. Note the deliberate distinction (enforced in CONTEXT.md): `Theme` is the global palette; a `ratatui::style::Style` is a single applied style — they are not synonyms.
