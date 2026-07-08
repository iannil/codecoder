# TUI keybinding and mode semantics

The TUI `Mode` is **derived, not stored**: `TuiApp` keeps no `mode` field. Each frame the active mode is computed from authoritative sub-state through a fixed precedence chain, and `Esc` unwinds that same stack one layer at a time.

## Derived mode

```
fn active_mode(app) -> Mode {
    if app.dialog.is_some()      { DIALOG }        // highest: blocking
    else if app.popup.is_some()  { SLASH / MODEL / ... per popup }
    else if app.search.is_active { SEARCH / R-SEARCH }
    else if app.browsing         { BROWSE }
    else                         { INSERT }        // fallback
}
```

A stored `mode: Mode` field (the common TUI approach) desyncs from reality whenever a transition is missed — e.g. a dialog closes but the field isn't reset, so the status bar and key handling disagree. Deriving it makes "exactly one mode per frame" a compile-time truth and keeps `Frame` read-only (render computes the mode, it never mutates it). The cost is one cheap recomputation per frame.

## Esc unwinds one layer

`Esc` peels the highest-precedence overlay/state, one press at a time — never a single-shot "clear everything":

- Dialog open → cancel the dialog (equivalent to deny/Cancel, answered via the `reply_tx` oneshot with a `Cancelled` variant — see [[0016-channel-topology-and-event-model]]).
- Popup open → dismiss the popup (no side effect), back to the underlying INSERT/SEARCH.
- In SEARCH/BROWSE with no overlay → exit that input state, back to INSERT.
- Already in INSERT → no-op. Esc does **not** clear the current input line; `Ctrl+U` does (terminal convention).
