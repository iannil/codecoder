// Transcript + zone rendering (ADR 0024). Fullscreen 3-zone layout with an
// optional activity line and a centered permission Dialog overlay.
use super::{Block as TBlock, Dialog, TuiApp, FOLD_THRESHOLD};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

pub fn draw(f: &mut Frame, app: &TuiApp) {
    let area = f.area();
    let input_h = (app.input.split('\n').count().clamp(1, 10)) as u16;
    let activity_h = if app.activity.is_some() { 1 } else { 0 };

    let zones = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),          // messages
            Constraint::Length(activity_h),
            Constraint::Length(input_h),
            Constraint::Length(1),       // status
        ])
        .split(area);

    draw_transcript(f, app, zones[0]);
    if activity_h == 1 {
        draw_activity(f, app, zones[1]);
    }
    draw_input(f, app, zones[2]);
    draw_status(f, app, zones[3]);

    // Completion popup floats just above the input zone.
    if app.popup.is_some() {
        draw_popup(f, app, zones[2]);
    }

    match &app.dialog {
        Some(Dialog::ToolPermission(_)) => draw_permission(f, app, area),
        Some(Dialog::AskQuestion(_)) => draw_ask(f, app, area),
        Some(Dialog::PlanApproval(_)) => draw_plan(f, app, area),
        Some(Dialog::Confirm(_)) => draw_confirm(f, app, area),
        Some(Dialog::Trust(_)) => draw_trust(f, app, area),
        None => {}
    }

    if app.help_open {
        draw_help(f, app, area);
    }

    // Verify mode replaces the normal 3-zone layout entirely.
    if app.active_mode() == super::Mode::Verify {
        crate::tui::verify::render_verify_dashboard(f, app, area);
        return;
    }
}

fn draw_help(f: &mut Frame, app: &TuiApp, area: Rect) {
    let t = &app.theme;
    let rows = [
        ("Enter", "send message"),
        ("Shift+Enter / Ctrl+J", "newline"),
        ("Ctrl+A / Ctrl+E", "line start / end"),
        ("Ctrl+W / Ctrl+K / Ctrl+U", "del word / kill to EOL / BOL"),
        ("Ctrl+←/→", "word left / right"),
        ("↑ (empty input)", "browse transcript"),
        ("Tab", "fold / unfold latest block"),
        ("Ctrl+F / Ctrl+R", "search / reverse search"),
        ("@path", "file completion · /  commands"),
        ("Mouse wheel", "scroll transcript"),
        ("F2", "toggle mouse capture (native copy)"),
        ("Esc", "close overlay / cancel"),
        ("Ctrl+C", "quit"),
    ];
    let w = 52u16.min(area.width.saturating_sub(4));
    let h = (rows.len() as u16 + 2).min(area.height.saturating_sub(2));
    let rect = centered(w, h, area);
    let lines: Vec<Line> = rows
        .iter()
        .map(|(k, d)| {
            Line::from(vec![
                Span::styled(format!(" {k:<24}"), Style::default().fg(t.accent)),
                Span::styled((*d).to_string(), Style::default().fg(t.fg)),
            ])
        })
        .collect();
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Keys — any key closes ")
        .border_style(Style::default().fg(t.accent));
    f.render_widget(Clear, rect);
    f.render_widget(Paragraph::new(lines).block(block), rect);
}

fn draw_popup(f: &mut Frame, app: &TuiApp, input_area: Rect) {
    let Some(popup) = &app.popup else { return };
    let t = &app.theme;
    let n = popup.items.len() as u16;
    let w = popup
        .items
        .iter()
        .map(|s| s.chars().count() as u16)
        .max()
        .unwrap_or(10)
        .saturating_add(4)
        .min(input_area.width);
    let h = (n + 2).min(input_area.y.max(1));
    let y = input_area.y.saturating_sub(h);
    let rect = Rect { x: input_area.x, y, width: w, height: h };

    let lines: Vec<Line> = popup
        .items
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let style = if i == popup.selected {
                Style::default().fg(t.accent).add_modifier(Modifier::REVERSED)
            } else {
                Style::default().fg(t.fg)
            };
            Line::from(Span::styled(format!(" {s} "), style))
        })
        .collect();

    let block = Block::default().borders(Borders::ALL).border_style(Style::default().fg(t.dim));
    f.render_widget(Clear, rect);
    f.render_widget(Paragraph::new(lines).block(block), rect);
}

fn draw_plan(f: &mut Frame, app: &TuiApp, area: Rect) {
    let Some(Dialog::PlanApproval(d)) = &app.dialog else { return };
    let t = &app.theme;
    let w = 64u16.min(area.width.saturating_sub(4));
    let plan_lines = d.plan.lines().count() as u16;
    let h = (plan_lines + 5).min(area.height.saturating_sub(2));
    let rect = centered(w, h, area);

    let mut lines: Vec<Line> = d.plan.lines().map(|l| Line::from(Span::styled(l.to_string(), Style::default().fg(t.fg)))).collect();
    lines.push(Line::from(""));
    let opts = [("a", "approve"), ("r", "reject")];
    let row: Vec<Span> = opts
        .iter()
        .enumerate()
        .flat_map(|(i, (k, label))| {
            let style = if i == d.selected {
                Style::default().fg(t.accent).add_modifier(Modifier::BOLD | Modifier::REVERSED)
            } else {
                Style::default().fg(t.fg)
            };
            vec![Span::styled(format!(" [{k}] {label} "), style), Span::raw("  ")]
        })
        .collect();
    lines.push(Line::from(row));

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Plan — approve? ")
        .border_style(Style::default().fg(t.accent));
    f.render_widget(Clear, rect);
    f.render_widget(Paragraph::new(lines).block(block).wrap(Wrap { trim: false }), rect);
}

fn draw_confirm(f: &mut Frame, app: &TuiApp, area: Rect) {
    let Some(Dialog::Confirm(d)) = &app.dialog else { return };
    let t = &app.theme;
    let w = 56u16.min(area.width.saturating_sub(4));
    let rect = centered(w, 5, area);
    let opts = [("y", "yes"), ("n", "no")];
    let row: Vec<Span> = opts
        .iter()
        .enumerate()
        .flat_map(|(i, (k, label))| {
            let style = if i == d.selected {
                Style::default().fg(t.accent).add_modifier(Modifier::BOLD | Modifier::REVERSED)
            } else {
                Style::default().fg(t.fg)
            };
            vec![Span::styled(format!(" [{k}] {label} "), style), Span::raw("  ")]
        })
        .collect();
    let lines = vec![
        Line::from(Span::styled(d.prompt.clone(), Style::default().fg(t.fg))),
        Line::from(""),
        Line::from(row),
    ];
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Confirm ")
        .border_style(Style::default().fg(t.accent));
    f.render_widget(Clear, rect);
    f.render_widget(Paragraph::new(lines).block(block).wrap(Wrap { trim: false }), rect);
}

fn draw_trust(f: &mut Frame, app: &TuiApp, area: Rect) {
    let Some(Dialog::Trust(d)) = &app.dialog else { return };
    let t = &app.theme;
    let w = 68u16.min(area.width.saturating_sub(4));
    let rect = centered(w, 7, area);
    let opts = [("a", "always"), ("o", "once"), ("n", "never")];
    let row: Vec<Span> = opts
        .iter()
        .enumerate()
        .flat_map(|(i, (k, label))| {
            let style = if i == d.selected {
                Style::default().fg(t.accent).add_modifier(Modifier::BOLD | Modifier::REVERSED)
            } else {
                Style::default().fg(t.fg)
            };
            vec![Span::styled(format!(" [{k}] {label} "), style), Span::raw("  ")]
        })
        .collect();
    let lines = vec![
        Line::from(Span::styled(
            format!("Trust this project's agent config? {}", d.root.display()),
            Style::default().fg(t.fg),
        )),
        Line::from(Span::styled(
            "Loads AGENTS.md, skills/prompts/capabilities, and codecoder.json.",
            Style::default().fg(t.fg),
        )),
        Line::from(""),
        Line::from(row),
    ];
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Trust project? ")
        .border_style(Style::default().fg(t.accent));
    f.render_widget(Clear, rect);
    f.render_widget(Paragraph::new(lines).block(block).wrap(Wrap { trim: false }), rect);
}

fn draw_ask(f: &mut Frame, app: &TuiApp, area: Rect) {
    let Some(Dialog::AskQuestion(d)) = &app.dialog else { return };
    let t = &app.theme;
    let w = 60u16.min(area.width.saturating_sub(4));
    let rect = centered(w, 6, area);
    let lines = vec![
        Line::from(Span::styled(d.prompt.clone(), Style::default().fg(t.fg))),
        Line::from(""),
        Line::from(vec![
            Span::styled("› ", Style::default().fg(t.accent)),
            Span::styled(d.input.clone(), Style::default().fg(t.fg)),
        ]),
    ];
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Question ")
        .border_style(Style::default().fg(t.accent));
    f.render_widget(Clear, rect);
    f.render_widget(Paragraph::new(lines).block(block).wrap(Wrap { trim: false }), rect);
}

fn draw_transcript(f: &mut Frame, app: &TuiApp, area: Rect) {
    let t = &app.theme;
    let sel_block = if app.browsing {
        app.collapsible_indices().get(app.browse_sel).copied()
    } else {
        None
    };

    let dim = Style::default().fg(t.dim);
    let accent = Style::default().fg(t.accent);
    let mut lines: Vec<Line> = Vec::new();

    for (i, block) in app.blocks.iter().enumerate() {
        let selected = Some(i) == sel_block;
        match block {
            TBlock::User(text) => push_prefixed(&mut lines, "you › ", accent, text, t.fg),
            TBlock::Assistant(text) => push_prefixed(&mut lines, "cc  · ", dim, text, t.fg),
            TBlock::System(text) => {
                lines.push(Line::from(Span::styled(format!("· {text}"), dim)))
            }
            TBlock::Reasoning { text, folded } => {
                let n = text.lines().count();
                let marker = if *folded { "▸" } else { "▾" };
                let mut head = Span::styled(format!("  {marker} reasoning ({n} lines)"), dim);
                if selected {
                    head = head.patch_style(Style::default().add_modifier(Modifier::REVERSED));
                }
                lines.push(Line::from(head));
                if !*folded {
                    for l in text.lines() {
                        lines.push(Line::from(Span::styled(format!("  │ {l}"), dim)));
                    }
                }
            }
            TBlock::Tool { name, preview, result, folded } => {
                let mut head = Line::from(vec![
                    Span::styled("  ▪ ", dim),
                    Span::styled(name.clone(), accent),
                    Span::raw("  "),
                    Span::styled(preview.clone(), dim),
                ]);
                if selected {
                    head = head.patch_style(Style::default().add_modifier(Modifier::REVERSED));
                }
                lines.push(head);
                if let Some(res) = result {
                    let style = if res.is_error {
                        Style::default().fg(t.error)
                    } else {
                        dim
                    };
                    let rlines: Vec<&str> = res.text.lines().collect();
                    let long = rlines.len() > FOLD_THRESHOLD;
                    if long && *folded {
                        let first = rlines.first().copied().unwrap_or("");
                        lines.push(Line::from(Span::styled(
                            format!("    └ {first} · {} lines ▸", rlines.len()),
                            style,
                        )));
                    } else {
                        for (j, l) in rlines.iter().enumerate() {
                            let lead = if j == 0 { "    └ " } else { "      " };
                            lines.push(Line::from(Span::styled(format!("{lead}{l}"), style)));
                        }
                    }
                }
            }
        }
        lines.push(Line::from("")); // blank separator between blocks
    }

    // Window the tail that fits (auto-scroll to bottom; browse adjusts app.scroll).
    let h = area.height as usize;
    let total = lines.len();
    let end = total.saturating_sub(app.scroll as usize);
    let start = end.saturating_sub(h);
    let visible: Vec<Line> = lines[start..end].to_vec();
    f.render_widget(Paragraph::new(visible).wrap(Wrap { trim: false }), area);
}

fn push_prefixed(lines: &mut Vec<Line>, prefix: &str, pstyle: Style, text: &str, fg: ratatui::style::Color) {
    let indent = " ".repeat(prefix.chars().count());
    for (i, l) in text.split('\n').enumerate() {
        if i == 0 {
            lines.push(Line::from(vec![
                Span::styled(prefix.to_string(), pstyle),
                Span::styled(l.to_string(), Style::default().fg(fg)),
            ]));
        } else {
            lines.push(Line::from(Span::styled(
                format!("{indent}{l}"),
                Style::default().fg(fg),
            )));
        }
    }
}

fn draw_activity(f: &mut Frame, app: &TuiApp, area: Rect) {
    let t = &app.theme;
    let Some(act) = &app.activity else { return };
    let spin = SPINNER[(app.frame_count as usize) % SPINNER.len()];
    let secs = act.started.elapsed().as_secs_f32();
    let line = Line::from(vec![
        Span::styled(format!("  {spin} "), Style::default().fg(t.accent)),
        Span::styled(act.label.clone(), Style::default().fg(t.fg)),
        Span::styled(format!("   {secs:.1}s   "), Style::default().fg(t.dim)),
        Span::styled("esc to cancel", Style::default().fg(t.dim)),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

fn draw_input(f: &mut Frame, app: &TuiApp, area: Rect) {
    let t = &app.theme;

    // Search bar replaces the input line while searching.
    if app.search_active {
        let label = if app.reverse_search { "r-search" } else { "search" };
        let line = Line::from(vec![
            Span::styled(format!("{label}> "), Style::default().fg(t.accent)),
            Span::styled(app.search_query.clone(), Style::default().fg(t.fg)),
        ]);
        f.render_widget(Paragraph::new(line), area);
        let x = area.x + label.chars().count() as u16 + 2 + app.search_query.chars().count() as u16;
        f.set_cursor_position((x, area.y));
        return;
    }

    let mut lines: Vec<Line> = Vec::new();
    for (i, l) in app.input.split('\n').enumerate() {
        let prefix = if i == 0 { "› " } else { "  " };
        lines.push(Line::from(vec![
            Span::styled(prefix, Style::default().fg(t.accent)),
            Span::styled(l.to_string(), Style::default().fg(t.fg)),
        ]));
    }
    if app.input.is_empty() {
        lines[0].spans.push(Span::styled(
            "type a message, or / for commands",
            Style::default().fg(t.dim),
        ));
    }
    f.render_widget(Paragraph::new(lines), area);

    // Cursor at its true (row, col) in the multiline input (ADR 0024).
    if app.dialog.is_none() && !app.browsing && !app.help_open {
        let (row, col) = app.cursor_rowcol();
        f.set_cursor_position((area.x + 2 + col, area.y + row));
    }
}

fn draw_status(f: &mut Frame, app: &TuiApp, area: Rect) {
    let t = &app.theme;
    let mode = app.active_mode();
    let ctx_style = if app.ctx_pct >= 75 {
        Style::default().fg(t.warn)
    } else {
        Style::default().fg(t.dim)
    };
    let hints = match mode {
        super::Mode::Browse => "↑/↓ select · tab fold · esc exit",
        super::Mode::Dialog => "↑/↓ select · enter confirm · esc deny",
        super::Mode::Verify => "Tab expand · ↑↓ select · F5 rerun · Esc exit",
        _ => "^J newline · ↑ browse · / cmd · ^C quit",
    };
    let line = Line::from(vec![
        Span::styled(format!(" {} ", mode.label()), Style::default().fg(t.accent).add_modifier(Modifier::BOLD)),
        Span::styled(format!(" {} ", app.model), Style::default().fg(t.dim)),
        Span::styled(format!(" ctx {}% ", app.ctx_pct), ctx_style),
        Span::styled(" · ", Style::default().fg(t.dim)),
        Span::styled(hints, Style::default().fg(t.dim)),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

fn draw_permission(f: &mut Frame, app: &TuiApp, area: Rect) {
    let Some(Dialog::ToolPermission(d)) = &app.dialog else { return };
    let t = &app.theme;
    let w = 54u16.min(area.width.saturating_sub(4));
    let h = (6 + d.options.len() as u16).min(area.height.saturating_sub(2));
    let rect = centered(w, h, area);

    let mut lines: Vec<Line> = vec![
        Line::from(Span::styled(d.preview.clone(), Style::default().fg(t.fg))),
        Line::from(Span::styled(format!("key: {}", d.key), Style::default().fg(t.dim))),
        Line::from(""),
    ];
    for (i, opt) in d.options.iter().enumerate() {
        let marker = if i == d.selected { "▸ " } else { "  " };
        let style = if i == d.selected {
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(t.fg)
        };
        lines.push(Line::from(vec![
            Span::styled(format!("{marker}[{}] {:<8}", opt.hotkey, opt.label), style),
            Span::styled(opt.hint.to_string(), Style::default().fg(t.dim)),
        ]));
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Permission ")
        .border_style(Style::default().fg(t.accent));
    f.render_widget(Clear, rect);
    f.render_widget(Paragraph::new(lines).block(block), rect);
}

fn centered(w: u16, h: u16, area: Rect) -> Rect {
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    Rect { x, y, width: w, height: h }
}

#[cfg(test)]
mod snapshot_tests {
    use super::*;
    use crate::tui::{TuiApp, Block, Dialog, Popup, PopupKind, PermissionDialog, AskDialog, PlanDialog, ConfirmDialog, TrustDialog, Activity, ToolResultView};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    /// 在 80×24 TestBackend 上渲染 app 并返回纯文本网格
    fn render_snapshot(app: &TuiApp) -> String {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
        terminal.backend().to_string()
    }
}
