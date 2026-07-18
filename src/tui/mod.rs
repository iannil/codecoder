// TUI state (ADR 0001, 0003, 0024). Mode is DERIVED each frame, never stored.
pub mod render;
pub mod run;

use crate::agent::{CancelToken, PermissionReply, TrustReply};
use crate::permission::PermScope;
use ratatui::style::Color;
use std::path::PathBuf;
use std::sync::mpsc::Sender;

pub const FOLD_THRESHOLD: usize = 6;
const SLASH_COMMANDS: [&str; 7] = ["/help", "/resume", "/reload", "/memory", "/clear", "/exit", "/quit"];
const MAX_COMPLETIONS: usize = 8;

/// The interaction context, computed from sub-state — see `TuiApp::active_mode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Insert,
    Search,
    RSearch,
    Dialog,
    Help,
    Model,
    Slash,
    Browse,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Insert => "INSERT",
            Mode::Search => "SEARCH",
            Mode::RSearch => "R-SEARCH",
            Mode::Dialog => "DIALOG",
            Mode::Help => "HELP",
            Mode::Model => "MODEL",
            Mode::Slash => "SLASH",
            Mode::Browse => "BROWSE",
        }
    }
}

/// One authorization option shown in a permission Dialog.
pub struct PermOption {
    pub hotkey: char,
    pub label: &'static str,
    pub hint: &'static str,
    pub reply: PermissionReply,
}

/// The ToolPermission dialog data (ADR 0016/0018). Holds the oneshot back-channel.
pub struct PermissionDialog {
    pub key: String,
    pub preview: String,
    pub options: Vec<PermOption>,
    pub selected: usize,
    pub reply_tx: Sender<PermissionReply>,
}

impl PermissionDialog {
    pub fn new(key: String, preview: String, reply_tx: Sender<PermissionReply>) -> Self {
        // Ceiling rule (ADR 0005/0022): @shell keys never offer AlwaysThisProject.
        let mut options = vec![PermOption {
            hotkey: 'o',
            label: "once",
            hint: "this call only",
            reply: PermissionReply::Grant(PermScope::Once),
        }];
        options.push(PermOption {
            hotkey: 's',
            label: "session",
            hint: "don't ask again this run",
            reply: PermissionReply::Grant(PermScope::AlwaysThisSession),
        });
        if !key.ends_with("@shell") {
            options.push(PermOption {
                hotkey: 'p',
                label: "project",
                hint: "persist to codecoder.json",
                reply: PermissionReply::Grant(PermScope::AlwaysThisProject),
            });
        }
        options.push(PermOption {
            hotkey: 'n',
            label: "deny",
            hint: "",
            reply: PermissionReply::Deny,
        });
        Self { key, preview, options, selected: 0, reply_tx }
    }
}

/// The agent's `ask_user` request: a text-input dialog whose answer flows back
/// over the oneshot reply_tx (ADR 0016).
pub struct AskDialog {
    pub prompt: String,
    pub input: String,
    pub reply_tx: Sender<String>,
}

/// The agent's `plan` proposal: approve/reject flows back as a bool (ADR 0016).
pub struct PlanDialog {
    pub plan: String,
    /// 0 = approve, 1 = reject.
    pub selected: usize,
    pub reply_tx: Sender<bool>,
}

/// A yes/no confirmation; the answer flows back as a bool (ADR 0016).
pub struct ConfirmDialog {
    pub prompt: String,
    /// 0 = yes, 1 = no.
    pub selected: usize,
    pub reply_tx: Sender<bool>,
}

/// A project-trust prompt (ADR 0028): trust this project's disk "self"?
/// `selected`: 0 = always, 1 = once, 2 = never.
pub struct TrustDialog {
    pub root: PathBuf,
    pub selected: usize,
    pub reply_tx: Sender<TrustReply>,
}

/// A blocking modal overlay (ADR 0016). While open, the derived Mode is Dialog.
pub enum Dialog {
    ToolPermission(PermissionDialog),
    PlanApproval(PlanDialog),
    AskQuestion(AskDialog),
    Confirm(ConfirmDialog),
    Trust(TrustDialog),
}

/// A non-blocking input-completion popup above the input (ADR 0024 / SLASH mode).
pub enum PopupKind {
    Slash,
    File,
}

pub struct Popup {
    pub kind: PopupKind,
    /// Full completion strings, e.g. `/resume` or `@src/main.rs`.
    pub items: Vec<String>,
    pub selected: usize,
}

/// One rendered element of the transcript.
pub enum Block {
    User(String),
    Assistant(String),
    Reasoning { text: String, folded: bool },
    Tool {
        name: String,
        preview: String,
        result: Option<ToolResultView>,
        folded: bool,
    },
    System(String),
}

pub struct ToolResultView {
    pub text: String,
    pub is_error: bool,
}

impl Block {
    /// A block the user can fold/expand: a Reasoning block, or a Tool whose
    /// result exceeds the fold threshold (ADR 0024 / CONTEXT DisplayState).
    pub fn is_collapsible(&self) -> bool {
        match self {
            Block::Reasoning { .. } => true,
            Block::Tool { result: Some(r), .. } => r.text.lines().count() > FOLD_THRESHOLD,
            _ => false,
        }
    }

    fn toggle_fold(&mut self) {
        match self {
            Block::Reasoning { folded, .. } => *folded = !*folded,
            Block::Tool { folded, .. } => *folded = !*folded,
            _ => {}
        }
    }
}

/// Flattened searchable text of a block.
pub fn block_text(b: &Block) -> String {
    match b {
        Block::User(s) | Block::Assistant(s) | Block::System(s) => s.clone(),
        Block::Reasoning { text, .. } => text.clone(),
        Block::Tool { name, preview, result, .. } => {
            let r = result.as_ref().map(|r| r.text.as_str()).unwrap_or("");
            format!("{name} {preview} {r}")
        }
    }
}

/// What the agent is currently doing, shown on the activity line while working.
pub struct Activity {
    pub label: String,
    pub started: std::time::Instant,
}

pub struct Theme {
    pub fg: Color,
    pub bg: Color,
    pub accent: Color,
    pub dim: Color,
    pub error: Color,
    pub warn: Color,
}

impl Theme {
    pub fn dark() -> Self {
        Self {
            fg: Color::White,
            bg: Color::Reset,
            accent: Color::Cyan,
            dim: Color::DarkGray,
            error: Color::Red,
            warn: Color::Yellow,
        }
    }
    pub fn light() -> Self {
        Self {
            fg: Color::Black,
            bg: Color::Reset,
            accent: Color::Blue,
            dim: Color::Gray,
            error: Color::Red,
            warn: Color::Yellow,
        }
    }
}

pub struct TuiApp {
    pub theme: Theme,
    pub frame_count: u64,
    pub model: String,
    pub root: PathBuf,
    pub ctx_pct: u16,

    pub blocks: Vec<Block>,
    /// True while the last Assistant block is still receiving stream deltas.
    pub streaming: bool,

    pub input: String,
    /// Cursor position as a char index into `input` (ADR 0024 readline editing).
    pub cursor: usize,
    pub history: Vec<String>,
    pub history_idx: Option<usize>,

    pub activity: Option<Activity>,
    pub dialog: Option<Dialog>,

    pub browsing: bool,
    /// Index into the collapsible-block list currently highlighted in BROWSE.
    pub browse_sel: usize,
    pub scroll: u16,
    /// Whether mouse capture is on (wheel scroll works); toggled with F2 so the
    /// user can drop to native terminal selection/copy (ADR 0024).
    pub mouse_captured: bool,

    pub popup: Option<Popup>,
    pub search_active: bool,
    pub reverse_search: bool,
    pub search_query: String,
    pub help_open: bool,

    /// Clone of the agent's cooperative-cancellation token (ADR 0016). Flipping
    /// it interrupts the in-flight turn directly — the command channel can't,
    /// because the agent loop blocks in `process_turn` and won't read it until
    /// the turn ends. Set by `run()`; default token in tests.
    pub cancel: CancelToken,

    pub should_quit: bool,
}

impl TuiApp {
    pub fn new(model: String, root: PathBuf) -> Self {
        Self {
            theme: Theme::dark(),
            frame_count: 0,
            model,
            root,
            ctx_pct: 0,
            blocks: Vec::new(),
            streaming: false,
            input: String::new(),
            cursor: 0,
            history: Vec::new(),
            history_idx: None,
            activity: None,
            dialog: None,
            browsing: false,
            browse_sel: 0,
            scroll: 0,
            mouse_captured: true,
            popup: None,
            search_active: false,
            reverse_search: false,
            search_query: String::new(),
            help_open: false,
            cancel: CancelToken::default(),
            should_quit: false,
        }
    }

    /// Enter SEARCH / R-SEARCH mode (Ctrl+F / Ctrl+R).
    pub fn begin_search(&mut self, reverse: bool) {
        self.search_active = true;
        self.reverse_search = reverse;
        self.search_query.clear();
        self.popup = None;
    }

    /// Jump into BROWSE at the first/last collapsible block matching the query,
    /// then leave search. Returns whether a match was found.
    pub fn commit_search(&mut self) -> bool {
        let q = self.search_query.to_lowercase();
        self.search_active = false;
        if q.is_empty() {
            return false;
        }
        let idxs = self.collapsible_indices();
        let matches: Vec<usize> = idxs
            .iter()
            .enumerate()
            .filter(|(_, bi)| block_text(&self.blocks[**bi]).to_lowercase().contains(&q))
            .map(|(sel, _)| sel)
            .collect();
        let hit = if self.reverse_search {
            matches.last().copied()
        } else {
            matches.first().copied()
        };
        if let Some(sel) = hit {
            self.browsing = true;
            self.browse_sel = sel;
            true
        } else {
            false
        }
    }

    /// Derived-mode precedence chain (ADR 0001): dialog → help → popup → search → browse → insert.
    pub fn active_mode(&self) -> Mode {
        if self.dialog.is_some() {
            Mode::Dialog
        } else if self.help_open {
            Mode::Help
        } else if self.popup.is_some() {
            Mode::Slash
        } else if self.search_active {
            if self.reverse_search { Mode::RSearch } else { Mode::Search }
        } else if self.browsing {
            Mode::Browse
        } else {
            Mode::Insert
        }
    }

    /// Indices of collapsible blocks, in order (BROWSE navigates these).
    pub fn collapsible_indices(&self) -> Vec<usize> {
        self.blocks
            .iter()
            .enumerate()
            .filter(|(_, b)| b.is_collapsible())
            .map(|(i, _)| i)
            .collect()
    }

    /// Toggle the fold of the most recent collapsible block (INSERT `Tab`).
    pub fn toggle_latest_fold(&mut self) {
        if let Some(&idx) = self.collapsible_indices().last() {
            self.blocks[idx].toggle_fold();
        }
    }

    /// Toggle the fold of the block currently selected in BROWSE.
    pub fn toggle_selected_fold(&mut self) {
        let idxs = self.collapsible_indices();
        if let Some(&idx) = idxs.get(self.browse_sel) {
            self.blocks[idx].toggle_fold();
        }
    }


    /// Recompute the completion popup from the current input (ADR 0024).
    /// `/…` at the start → slash commands; a token starting with `@` → files.
    pub fn refresh_popup(&mut self) {
        self.popup = self.compute_popup();
    }

    fn compute_popup(&self) -> Option<Popup> {
        let last_line = self.input.split('\n').last().unwrap_or("");
        let last_tok = last_line.rsplit(' ').next().unwrap_or("");

        // Slash: only when the whole input is a single `/…` token.
        if self.input.starts_with('/') && !self.input.contains(char::is_whitespace) {
            let items: Vec<String> = SLASH_COMMANDS
                .iter()
                .filter(|c| c.starts_with(&self.input))
                .map(|c| c.to_string())
                .collect();
            return (!items.is_empty()).then_some(Popup { kind: PopupKind::Slash, items, selected: 0 });
        }

        // File: a token like `@path/frag`.
        if let Some(frag) = last_tok.strip_prefix('@') {
            let (dir, prefix) = match frag.rsplit_once('/') {
                Some((d, p)) => (d.to_string(), p.to_string()),
                None => (String::new(), frag.to_string()),
            };
            let mut items = Vec::new();
            if let Ok(entries) = std::fs::read_dir(self.root.join(&dir)) {
                for e in entries.flatten() {
                    let mut name = e.file_name().to_string_lossy().into_owned();
                    if name.starts_with('.') || !name.starts_with(&prefix) {
                        continue;
                    }
                    if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                        name.push('/');
                    }
                    let full = if dir.is_empty() { name } else { format!("{dir}/{name}") };
                    items.push(format!("@{full}"));
                    if items.len() >= MAX_COMPLETIONS {
                        break;
                    }
                }
            }
            items.sort();
            return (!items.is_empty()).then_some(Popup { kind: PopupKind::File, items, selected: 0 });
        }
        None
    }

    #[cfg(test)]
    fn probe() -> Self {
        Self::new("m".into(), std::env::temp_dir())
    }

    // --- readline-style input editing (cursor is a char index into `input`) ---

    fn chars(&self) -> Vec<char> {
        self.input.chars().collect()
    }
    fn rebuild(&mut self, v: Vec<char>) {
        self.input = v.into_iter().collect();
    }

    pub fn set_input(&mut self, s: String) {
        self.cursor = s.chars().count();
        self.input = s;
    }
    pub fn clear_input(&mut self) {
        self.input.clear();
        self.cursor = 0;
    }
    pub fn insert_char(&mut self, c: char) {
        let mut v = self.chars();
        v.insert(self.cursor, c);
        self.cursor += 1;
        self.rebuild(v);
    }
    pub fn insert_str(&mut self, s: &str) {
        for c in s.chars() {
            self.insert_char(c);
        }
    }
    pub fn backspace(&mut self) {
        if self.cursor > 0 {
            let mut v = self.chars();
            v.remove(self.cursor - 1);
            self.cursor -= 1;
            self.rebuild(v);
        }
    }
    pub fn move_left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }
    pub fn move_right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.chars().len());
    }
    /// Start of the current line (Ctrl+A).
    pub fn move_home(&mut self) {
        let v = self.chars();
        while self.cursor > 0 && v[self.cursor - 1] != '\n' {
            self.cursor -= 1;
        }
    }
    /// End of the current line (Ctrl+E).
    pub fn move_end(&mut self) {
        let v = self.chars();
        while self.cursor < v.len() && v[self.cursor] != '\n' {
            self.cursor += 1;
        }
    }
    pub fn move_word_left(&mut self) {
        let v = self.chars();
        while self.cursor > 0 && v[self.cursor - 1].is_whitespace() {
            self.cursor -= 1;
        }
        while self.cursor > 0 && !v[self.cursor - 1].is_whitespace() {
            self.cursor -= 1;
        }
    }
    pub fn move_word_right(&mut self) {
        let v = self.chars();
        let n = v.len();
        while self.cursor < n && v[self.cursor].is_whitespace() {
            self.cursor += 1;
        }
        while self.cursor < n && !v[self.cursor].is_whitespace() {
            self.cursor += 1;
        }
    }
    /// Delete the word before the cursor (Ctrl+W).
    pub fn delete_word(&mut self) {
        let start_cursor = self.cursor;
        self.move_word_left();
        let mut v = self.chars();
        v.drain(self.cursor..start_cursor);
        self.rebuild(v);
    }
    /// Kill from cursor to end of current line (Ctrl+K).
    pub fn kill_to_eol(&mut self) {
        let end = {
            let start = self.cursor;
            self.move_end();
            let e = self.cursor;
            self.cursor = start;
            e
        };
        let mut v = self.chars();
        v.drain(self.cursor..end);
        self.rebuild(v);
    }
    /// Kill from start of current line to cursor (Ctrl+U).
    pub fn kill_to_bol(&mut self) {
        let end = self.cursor;
        self.move_home();
        let mut v = self.chars();
        v.drain(self.cursor..end);
        self.rebuild(v);
    }
    /// Cursor position as (row, col) in the multiline input, for rendering.
    pub fn cursor_rowcol(&self) -> (u16, u16) {
        let v = self.chars();
        let mut row = 0u16;
        let mut col = 0u16;
        for &c in v.iter().take(self.cursor) {
            if c == '\n' {
                row += 1;
                col = 0;
            } else {
                col += 1;
            }
        }
        (row, col)
    }

    /// Complete the current token with the popup's selection, then dismiss it.
    pub fn accept_popup(&mut self) {
        let Some(popup) = self.popup.take() else { return };
        let Some(item) = popup.items.get(popup.selected).cloned() else { return };
        match popup.kind {
            PopupKind::Slash => self.set_input(item),
            PopupKind::File => {
                // Replace the trailing `@token` with the chosen path.
                if let Some(at) = self.input.rfind('@') {
                    let mut base = self.input[..at].to_string();
                    base.push_str(&item);
                    self.set_input(base);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slash_popup_filters_and_completes() {
        let mut app = TuiApp::probe();
        app.input = "/re".into();
        app.refresh_popup();
        let p = app.popup.as_ref().expect("slash popup");
        assert!(p.items.iter().any(|i| i == "/resume"));
        app.popup.as_mut().unwrap().selected =
            p.items.iter().position(|i| i == "/resume").unwrap();
        app.accept_popup();
        assert_eq!(app.input, "/resume");
        assert!(app.popup.is_none());
    }

    #[test]
    fn file_popup_completes_at_token() {
        let dir = std::env::temp_dir().join(format!("cc_pop_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("notes.txt"), "x").unwrap();

        let mut app = TuiApp::new("m".into(), dir.clone());
        app.input = "look at @not".into();
        app.refresh_popup();
        let p = app.popup.as_ref().expect("file popup");
        assert!(p.items.iter().any(|i| i == "@notes.txt"), "{:?}", p.items);
        app.accept_popup();
        assert_eq!(app.input, "look at @notes.txt");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn plain_text_opens_no_popup() {
        let mut app = TuiApp::probe();
        app.input = "hello world".into();
        app.refresh_popup();
        assert!(app.popup.is_none());
    }

    #[test]
    fn readline_edits() {
        let mut app = TuiApp::probe();
        app.insert_str("hello world");
        assert_eq!(app.cursor, 11);
        app.move_home();
        assert_eq!(app.cursor, 0);
        app.move_word_right(); // past "hello"
        assert_eq!(app.cursor, 5);
        app.move_end();
        app.delete_word(); // removes "world"
        assert_eq!(app.input, "hello ");
        app.kill_to_bol();
        assert_eq!(app.input, "");
        assert_eq!(app.cursor, 0);
    }

    #[test]
    fn readline_multiline_cursor_rowcol() {
        let mut app = TuiApp::probe();
        app.insert_str("ab\ncde");
        assert_eq!(app.cursor_rowcol(), (1, 3));
        app.move_home();
        assert_eq!(app.cursor_rowcol(), (1, 0));
    }

    #[test]
    fn search_jumps_to_matching_block() {
        let mut app = TuiApp::probe();
        app.blocks.push(Block::Reasoning { text: "alpha".into(), folded: true });
        app.blocks.push(Block::Reasoning { text: "beta gamma".into(), folded: true });
        app.begin_search(false);
        app.search_query = "gamma".into();
        assert!(app.commit_search());
        assert!(app.browsing);
        assert_eq!(app.browse_sel, 1);
    }

    #[test]
    fn tab_toggles_latest_collapsible() {
        let mut app = TuiApp::probe();
        app.blocks.push(Block::Reasoning { text: "a\nb".into(), folded: true });
        app.toggle_latest_fold();
        match &app.blocks[0] {
            Block::Reasoning { folded, .. } => assert!(!folded),
            _ => panic!(),
        }
    }
}
