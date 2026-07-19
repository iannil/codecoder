// TUI run loop (ADR 0024): fullscreen alternate screen, unified channel with an
// input thread + AgentEvent forwarder + animation tick, blocking recv (0 idle CPU).
use super::{Activity, AskDialog, Block, ConfirmDialog, Dialog, PermissionDialog, PlanDialog, ToolResultView, TrustDialog, TuiApp};
use crate::agent::{AgentCommand, AgentEvent, CancelToken, SteerQueue, TrustReply};
use std::path::PathBuf;
use crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags, MouseEventKind,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use std::io::{self, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;
use std::time::Duration;

enum TuiMsg {
    Input(Event),
    Agent(AgentEvent),
    Tick,
}

pub fn run(
    model: String,
    root: PathBuf,
    cmd_tx: Sender<AgentCommand>,
    event_rx: Receiver<AgentEvent>,
    cancel: CancelToken,
    steer: SteerQueue,
) -> anyhow::Result<()> {
    // --- terminal setup ---
    enable_raw_mode()?;
    let mut out = io::stdout();
    crossterm::execute!(out, EnterAlternateScreen, EnableBracketedPaste, EnableMouseCapture)?;
    // Best-effort: enables reliable Shift+Enter etc. on supporting terminals (ADR 0024).
    let _ = crossterm::execute!(
        out,
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
    );
    let mut terminal = Terminal::new(CrosstermBackend::new(out))?;

    // --- unified channel: input thread + agent forwarder + tick ---
    let (tx, rx) = channel::<TuiMsg>();
    let animating = Arc::new(AtomicBool::new(false));

    {
        let tx = tx.clone();
        thread::spawn(move || {
            while let Ok(ev) = event::read() {
                if tx.send(TuiMsg::Input(ev)).is_err() {
                    break;
                }
            }
        });
    }
    {
        let tx = tx.clone();
        thread::spawn(move || {
            for ev in event_rx {
                if tx.send(TuiMsg::Agent(ev)).is_err() {
                    break;
                }
            }
        });
    }
    {
        let tx = tx.clone();
        let animating = Arc::clone(&animating);
        thread::spawn(move || {
            loop {
                thread::sleep(Duration::from_millis(50));
                if animating.load(Ordering::Relaxed) && tx.send(TuiMsg::Tick).is_err() {
                    break;
                }
            }
        });
    }

    // --- main loop: draw, then block on the next message ---
    let mut app = TuiApp::new(model, root);
    app.cancel = cancel;
    app.steer = steer;
    let result = (|| -> anyhow::Result<()> {
        loop {
            terminal.draw(|f| super::render::draw(f, &app))?;
            app.frame_count = app.frame_count.wrapping_add(1);

            match rx.recv()? {
                TuiMsg::Input(ev) => handle_input(&mut app, ev, &cmd_tx),
                TuiMsg::Agent(ev) => handle_agent(&mut app, ev),
                TuiMsg::Tick => {}
            }
            animating.store(app.activity.is_some(), Ordering::Relaxed);
            if app.should_quit {
                break;
            }
        }
        Ok(())
    })();

    // --- teardown (always) ---
    let mut out = io::stdout();
    let _ = crossterm::execute!(out, PopKeyboardEnhancementFlags, DisableMouseCapture, DisableBracketedPaste, LeaveAlternateScreen);
    let _ = disable_raw_mode();
    let _ = out.flush();
    result
}

fn handle_agent(app: &mut TuiApp, ev: AgentEvent) {
    match ev {
        AgentEvent::StreamDelta(s) => {
            if !app.streaming || !matches!(app.blocks.last(), Some(Block::Assistant(_))) {
                app.blocks.push(Block::Assistant(String::new()));
                app.streaming = true;
            }
            if let Some(Block::Assistant(text)) = app.blocks.last_mut() {
                text.push_str(&s);
            }
            app.activity = None;
        }
        AgentEvent::Reasoning(text) => {
            app.blocks.push(Block::Reasoning { text, folded: true });
        }
        AgentEvent::ToolStarted { name, preview } => {
            app.activity = Some(Activity {
                label: format!("running {name}…"),
                started: std::time::Instant::now(),
            });
            app.blocks.push(Block::Tool { name, preview, result: None, folded: true });
            app.streaming = false;
        }
        AgentEvent::ToolFinished { name, is_error, output } => {
            for block in app.blocks.iter_mut().rev() {
                if let Block::Tool { name: n, result, folded, .. } = block {
                    if *n == name && result.is_none() {
                        *folded = output.lines().count() > super::FOLD_THRESHOLD;
                        *result = Some(ToolResultView { text: output, is_error });
                        break;
                    }
                }
            }
        }
        AgentEvent::SubAgentMilestone(m) => app.blocks.push(Block::System(format!("sub-agent: {m}"))),
        AgentEvent::Context { pct } => app.ctx_pct = pct,
        AgentEvent::Notice(m) => app.blocks.push(Block::System(m)),
        AgentEvent::PermissionRequest { key, preview, reply_tx } => {
            app.dialog = Some(Dialog::ToolPermission(PermissionDialog::new(key, preview, reply_tx)));
        }
        AgentEvent::AskUser { prompt, reply_tx } => {
            app.dialog = Some(Dialog::AskQuestion(AskDialog { prompt, input: String::new(), reply_tx }));
        }
        AgentEvent::PlanApproval { plan, reply_tx } => {
            app.dialog = Some(Dialog::PlanApproval(PlanDialog { plan, selected: 0, reply_tx }));
        }
        AgentEvent::Confirm { prompt, reply_tx } => {
            app.dialog = Some(Dialog::Confirm(ConfirmDialog { prompt, selected: 0, reply_tx }));
        }
        AgentEvent::TrustPrompt { root, reply_tx } => {
            app.dialog = Some(Dialog::Trust(TrustDialog { root, selected: 0, reply_tx }));
        }
        AgentEvent::TurnComplete => {
            app.streaming = false;
            app.activity = None;
        }
    }
}

fn handle_input(app: &mut TuiApp, ev: Event, cmd_tx: &Sender<AgentCommand>) {
    match ev {
        Event::Paste(s) => {
            app.insert_str(&s);
            app.refresh_popup();
        }
        Event::Mouse(me) => match me.kind {
            MouseEventKind::ScrollUp => app.scroll = app.scroll.saturating_add(3),
            MouseEventKind::ScrollDown => app.scroll = app.scroll.saturating_sub(3),
            _ => {}
        },
        Event::Key(k) if k.kind == KeyEventKind::Press || k.kind == KeyEventKind::Repeat => {
            match &app.dialog {
                Some(Dialog::ToolPermission(_)) => handle_permission_key(app, k),
                Some(Dialog::AskQuestion(_)) => handle_ask_key(app, k),
                Some(Dialog::PlanApproval(_)) => handle_plan_key(app, k),
                Some(Dialog::Confirm(_)) => handle_confirm_key(app, k),
                Some(Dialog::Trust(_)) => handle_trust_key(app, k),
                None if app.help_open => handle_help_key(app, k),
                None if app.search_active => handle_search_key(app, k),
                None if app.browsing => handle_browse_key(app, k),
                None => handle_insert_key(app, k, cmd_tx),
            }
        }
        _ => {}
    }
}

fn handle_help_key(app: &mut TuiApp, _k: KeyEvent) {
    // Any key closes the help overlay (ADR 0001 Esc unwind; also F1/others).
    app.help_open = false;
}

fn handle_search_key(app: &mut TuiApp, k: KeyEvent) {
    match k.code {
        KeyCode::Esc => {
            app.search_active = false;
            app.search_query.clear();
        }
        KeyCode::Enter => {
            app.commit_search();
        }
        KeyCode::Backspace => {
            app.search_query.pop();
        }
        KeyCode::Char(c) => app.search_query.push(c),
        _ => {}
    }
}

fn handle_ask_key(app: &mut TuiApp, k: KeyEvent) {
    let Some(Dialog::AskQuestion(d)) = &mut app.dialog else { return };
    match k.code {
        KeyCode::Enter => {
            if let Some(Dialog::AskQuestion(d)) = app.dialog.take() {
                let _ = d.reply_tx.send(d.input);
            }
        }
        KeyCode::Esc => {
            if let Some(Dialog::AskQuestion(d)) = app.dialog.take() {
                let _ = d.reply_tx.send(String::new());
            }
        }
        KeyCode::Backspace => {
            d.input.pop();
        }
        KeyCode::Char(c) => d.input.push(c),
        _ => {}
    }
}

fn handle_plan_key(app: &mut TuiApp, k: KeyEvent) {
    let Some(Dialog::PlanApproval(d)) = &mut app.dialog else { return };
    let answer = |app: &mut TuiApp, approved: bool| {
        if let Some(Dialog::PlanApproval(d)) = app.dialog.take() {
            let _ = d.reply_tx.send(approved);
        }
    };
    match k.code {
        KeyCode::Up | KeyCode::Down => d.selected = 1 - d.selected,
        KeyCode::Char('a') | KeyCode::Char('A') => answer(app, true),
        KeyCode::Char('r') | KeyCode::Char('R') | KeyCode::Esc => answer(app, false),
        KeyCode::Enter => {
            let approved = d.selected == 0;
            answer(app, approved);
        }
        _ => {}
    }
}

fn handle_confirm_key(app: &mut TuiApp, k: KeyEvent) {
    let Some(Dialog::Confirm(d)) = &mut app.dialog else { return };
    let answer = |app: &mut TuiApp, yes: bool| {
        if let Some(Dialog::Confirm(d)) = app.dialog.take() {
            let _ = d.reply_tx.send(yes);
        }
    };
    match k.code {
        KeyCode::Up | KeyCode::Down => d.selected = 1 - d.selected,
        KeyCode::Char('y') | KeyCode::Char('Y') => answer(app, true),
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => answer(app, false),
        KeyCode::Enter => {
            let yes = d.selected == 0;
            answer(app, yes);
        }
        _ => {}
    }
}

fn handle_trust_key(app: &mut TuiApp, k: KeyEvent) {
    let Some(Dialog::Trust(d)) = &mut app.dialog else { return };
    // 0 = always, 1 = once, 2 = never.
    let answer = |app: &mut TuiApp, reply: TrustReply| {
        if let Some(Dialog::Trust(d)) = app.dialog.take() {
            let _ = d.reply_tx.send(reply);
        }
    };
    match k.code {
        KeyCode::Left | KeyCode::Up => d.selected = d.selected.saturating_sub(1),
        KeyCode::Right | KeyCode::Down => d.selected = (d.selected + 1).min(2),
        KeyCode::Char('a') | KeyCode::Char('A') => answer(app, TrustReply::Always),
        KeyCode::Char('o') | KeyCode::Char('O') => answer(app, TrustReply::Once),
        // Esc defaults to the safe choice: don't trust.
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => answer(app, TrustReply::Never),
        KeyCode::Enter => {
            let reply = match d.selected {
                0 => TrustReply::Always,
                1 => TrustReply::Once,
                _ => TrustReply::Never,
            };
            answer(app, reply);
        }
        _ => {}
    }
}

fn handle_permission_key(app: &mut TuiApp, k: KeyEvent) {
    let Some(Dialog::ToolPermission(d)) = &mut app.dialog else { return };
    let choose = |idx: usize, app: &mut TuiApp| {
        if let Some(Dialog::ToolPermission(d)) = app.dialog.take() {
            if let Some(opt) = d.options.into_iter().nth(idx) {
                let _ = d.reply_tx.send(opt.reply);
            }
        }
    };
    match k.code {
        KeyCode::Up => d.selected = d.selected.saturating_sub(1),
        KeyCode::Down => d.selected = (d.selected + 1).min(d.options.len() - 1),
        KeyCode::Enter => {
            let idx = d.selected;
            choose(idx, app);
        }
        KeyCode::Esc => {
            // Deny is always the last option (ADR 0001: Esc cancels the dialog).
            let idx = d.options.len() - 1;
            choose(idx, app);
        }
        KeyCode::Char(c) => {
            if let Some(idx) = d.options.iter().position(|o| o.hotkey == c) {
                choose(idx, app);
            }
        }
        _ => {}
    }
}

fn handle_browse_key(app: &mut TuiApp, k: KeyEvent) {
    let count = app.collapsible_indices().len();
    match k.code {
        KeyCode::Up => app.browse_sel = app.browse_sel.saturating_sub(1),
        KeyCode::Down => {
            if count > 0 {
                app.browse_sel = (app.browse_sel + 1).min(count - 1);
            }
        }
        KeyCode::Tab | KeyCode::Enter => app.toggle_selected_fold(),
        KeyCode::Esc => app.browsing = false,
        KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => app.should_quit = true,
        _ => {}
    }
}

fn handle_insert_key(app: &mut TuiApp, k: KeyEvent, cmd_tx: &Sender<AgentCommand>) {
    // While a completion popup is open, these keys drive it (ADR 0024).
    if let Some(popup) = &mut app.popup {
        match k.code {
            KeyCode::Up => {
                popup.selected = popup.selected.saturating_sub(1);
                return;
            }
            KeyCode::Down => {
                popup.selected = (popup.selected + 1).min(popup.items.len() - 1);
                return;
            }
            KeyCode::Tab | KeyCode::Enter => {
                app.accept_popup();
                return;
            }
            KeyCode::Esc => {
                app.popup = None;
                return;
            }
            _ => {}
        }
    }

    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    let shift = k.modifiers.contains(KeyModifiers::SHIFT);
    match k.code {
        KeyCode::Enter if shift => app.insert_char('\n'),
        KeyCode::Char('j') if ctrl => app.insert_char('\n'),
        KeyCode::Enter => submit(app, cmd_tx),
        // Esc while the agent is working cancels the in-flight turn by flipping
        // the shared token directly (ADR 0016) — the command channel can't reach
        // the agent loop mid-turn. With no turn active, Esc is a no-op here.
        KeyCode::Esc if app.activity.is_some() => app.cancel.cancel(),
        KeyCode::Char('c') if ctrl => app.should_quit = true,
        KeyCode::Char('f') if ctrl => app.begin_search(false),
        KeyCode::Char('r') if ctrl => app.begin_search(true),
        KeyCode::Char('a') if ctrl => app.move_home(),
        KeyCode::Char('e') if ctrl => app.move_end(),
        KeyCode::Char('u') if ctrl => app.kill_to_bol(),
        KeyCode::Char('k') if ctrl => app.kill_to_eol(),
        KeyCode::Char('w') if ctrl => app.delete_word(),
        KeyCode::Left if ctrl => app.move_word_left(),
        KeyCode::Right if ctrl => app.move_word_right(),
        KeyCode::Left => app.move_left(),
        KeyCode::Right => app.move_right(),
        KeyCode::Home => app.move_home(),
        KeyCode::End => app.move_end(),
        KeyCode::F(1) => app.help_open = true,
        KeyCode::F(2) => {
            // Toggle mouse capture so the user can drop to native selection/copy.
            app.mouse_captured = !app.mouse_captured;
            let mut out = std::io::stdout();
            if app.mouse_captured {
                let _ = crossterm::execute!(out, EnableMouseCapture);
            } else {
                let _ = crossterm::execute!(out, DisableMouseCapture);
            }
        }
        KeyCode::Backspace => app.backspace(),
        KeyCode::Tab => app.toggle_latest_fold(),
        KeyCode::Up => {
            if app.input.is_empty() {
                let n = app.collapsible_indices().len();
                if n > 0 {
                    app.browsing = true;
                    app.browse_sel = n - 1;
                }
            } else {
                history_prev(app);
            }
        }
        KeyCode::Down => history_next(app),
        KeyCode::Char(c) => app.insert_char(c),
        _ => {}
    }
    // Recompute completion popup from the (possibly) edited input.
    app.refresh_popup();
}

fn submit(app: &mut TuiApp, cmd_tx: &Sender<AgentCommand>) {
    let text = app.input.trim().to_string();
    app.clear_input();
    if text.is_empty() {
        return;
    }
    app.history.push(text.clone());
    app.history_idx = None;

    if let Some(cmd) = text.strip_prefix('/') {
        match cmd {
            "exit" | "quit" => {
                let _ = cmd_tx.send(AgentCommand::Shutdown);
                app.should_quit = true;
            }
            "resume" => {
                let _ = cmd_tx.send(AgentCommand::Resume);
            }
            "reload" => {
                let _ = cmd_tx.send(AgentCommand::Reload);
            }
            "clear" => {
                app.blocks.clear();
                app.browsing = false;
                let _ = cmd_tx.send(AgentCommand::Clear);
            }
            "memory" => {
                let keys = crate::memory::list(&app.root);
                let body = if keys.is_empty() { "(no memories)".into() } else { keys.join("  ") };
                app.blocks.push(Block::System(format!("memory: {body}")));
            }
            "help" => app.help_open = true,
            other => app.blocks.push(Block::System(format!("unknown command /{other}"))),
        }
        return;
    }

    app.blocks.push(Block::User(text.clone()));

    // A turn is already in flight → steer it (ADR 0029) instead of queuing a new
    // ProcessMessage: push to the shared queue the running turn drains. The
    // command channel can't deliver mid-turn (the agent blocks in process_turn).
    if app.activity.is_some() {
        app.steer.push(text);
        return;
    }

    app.activity = Some(Activity {
        label: "thinking…".into(),
        started: std::time::Instant::now(),
    });
    let _ = cmd_tx.send(AgentCommand::ProcessMessage(text));
}

fn history_prev(app: &mut TuiApp) {
    if app.history.is_empty() {
        return;
    }
    let idx = match app.history_idx {
        None => app.history.len() - 1,
        Some(i) => i.saturating_sub(1),
    };
    app.history_idx = Some(idx);
    let text = app.history[idx].clone();
    app.set_input(text);
}

fn history_next(app: &mut TuiApp) {
    let Some(i) = app.history_idx else { return };
    if i + 1 < app.history.len() {
        app.history_idx = Some(i + 1);
        let text = app.history[i + 1].clone();
        app.set_input(text);
    } else {
        app.history_idx = None;
        app.clear_input();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    fn app_with_token() -> (TuiApp, CancelToken) {
        let mut app = TuiApp::new("m".into(), std::env::temp_dir());
        let token = CancelToken::default();
        app.cancel = token.clone();
        (app, token)
    }

    #[test]
    fn esc_during_activity_flips_the_cancel_token() {
        let (mut app, token) = app_with_token();
        app.activity = Some(Activity {
            label: "working".into(),
            started: std::time::Instant::now(),
        });
        let (tx, _rx) = channel::<AgentCommand>();
        handle_insert_key(&mut app, KeyEvent::from(KeyCode::Esc), &tx);
        assert!(token.is_cancelled(), "Esc while a turn is active must cancel it");
    }

    #[test]
    fn esc_with_no_active_turn_does_not_cancel() {
        let (mut app, token) = app_with_token();
        app.activity = None;
        let (tx, _rx) = channel::<AgentCommand>();
        handle_insert_key(&mut app, KeyEvent::from(KeyCode::Esc), &tx);
        assert!(
            !token.is_cancelled(),
            "Esc with no turn in progress must be a no-op"
        );
    }

    #[test]
    fn submit_while_turn_in_flight_steers_instead_of_queuing() {
        let (mut app, _token) = app_with_token();
        let steer = SteerQueue::default();
        app.steer = steer.clone();
        app.activity = Some(Activity { label: "working".into(), started: std::time::Instant::now() });
        app.input = "focus on the parser".into();
        let (tx, rx) = channel::<AgentCommand>();

        submit(&mut app, &tx);

        // Steered, not queued as a new ProcessMessage.
        assert_eq!(steer.drain(), vec!["focus on the parser".to_string()]);
        assert!(rx.try_recv().is_err(), "no ProcessMessage should be sent while a turn is in flight");
    }

    #[test]
    fn submit_with_no_turn_sends_process_message() {
        let (mut app, _token) = app_with_token();
        let steer = SteerQueue::default();
        app.steer = steer.clone();
        app.activity = None;
        app.input = "hello".into();
        let (tx, rx) = channel::<AgentCommand>();

        submit(&mut app, &tx);

        assert!(steer.drain().is_empty(), "nothing should be steered when idle");
        assert!(matches!(rx.try_recv(), Ok(AgentCommand::ProcessMessage(m)) if m == "hello"));
    }
}
