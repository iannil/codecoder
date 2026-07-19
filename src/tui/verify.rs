// TUI verify dashboard rendering (Mode::VERIFY).
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use crate::tui::Theme;
use crate::verify::{CaseStatus, VerifyFocus, VerifyState};

const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// Maximum lines of output to show per failed test case.
const MAX_OUTPUT_LINES: usize = 10;

/// Render the verify dashboard into the given frame.
pub fn render_verify_dashboard(f: &mut Frame, app: &crate::tui::TuiApp, area: Rect) {
    let t = &app.theme;
    let verify = &app.verify_state;

    // Layout: title + layers + summary + shortcuts
    let zones = Layout::default()
        .direction(ratatui::layout::Direction::Vertical)
        .constraints([
            Constraint::Length(1),    // title
            Constraint::Min(1),       // layers
            Constraint::Length(2),    // summary
            Constraint::Length(1),    // shortcuts
        ])
        .split(area);

    // Title bar
    render_title(f, verify, t, zones[0]);

    // Layer/module/case list
    render_layers(f, verify, t, zones[1], app.frame_count);

    // Summary
    render_summary(f, verify, t, zones[2]);

    // Shortcuts
    render_shortcuts(f, t, zones[3]);
}

fn render_title(f: &mut Frame, state: &VerifyState, t: &Theme, area: Rect) {
    let status = if state.running {
        let spin = SPINNER[(std::time::Instant::now().elapsed().as_millis() as usize / 100) % SPINNER.len()];
        format!(" {spin} 运行中 {:.1}s", state.started_at.elapsed().as_secs_f64())
    } else if state.cancelled {
        " 已取消".to_string()
    } else if state.error.is_some() {
        " 错误".to_string()
    } else {
        " 完成".to_string()
    };
    let status_style = if state.running {
        Style::default().fg(t.warn)
    } else if state.failed > 0 {
        Style::default().fg(t.error)
    } else {
        Style::default().fg(t.accent)
    };
    let line = Line::from(vec![
        Span::styled(" CodeCoder 验证仪表盘 ", Style::default().fg(t.fg).add_modifier(Modifier::BOLD)),
        Span::styled("·", Style::default().fg(t.dim)),
        Span::styled(status, status_style),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

fn render_layers(f: &mut Frame, state: &VerifyState, t: &Theme, area: Rect, frame_count: u64) {
    let mut lines: Vec<Line> = Vec::new();

    for (layer_idx, layer) in state.layers.iter().enumerate() {
        // Layer header with progress bar
        let total = layer.passed + layer.failed + layer.skipped;
        let pct = if total > 0 { (layer.passed * 100) / total } else { 0 };

        let is_focused = matches!(state.focus, VerifyFocus::Layer(i) if i == layer_idx);
        let header_style = if is_focused {
            Style::default().fg(t.accent).add_modifier(Modifier::REVERSED)
        } else {
            Style::default().fg(t.fg)
        };

        let layer_icon = if layer.folded { "▸" } else { "▾" };
        let status_icon = if layer.failed > 0 { "✗" } else if layer.passed > 0 { "✔" } else { "⏸" };

        let header = format!(
            "  {layer_icon}  [{status_icon}] {name}  {passed}/{total}  {pct}%",
            name = layer.name,
            passed = layer.passed,
            total = total,
            pct = pct,
        );
        lines.push(Line::from(Span::styled(header, header_style)));

        if !layer.folded {
            for (mod_idx, module) in layer.modules.iter().enumerate() {
                let mod_focused = matches!(state.focus, VerifyFocus::Module { layer: l, module: m } if l == layer_idx && m == mod_idx);
                let mod_style = if mod_focused {
                    Style::default().fg(t.accent).add_modifier(Modifier::REVERSED)
                } else {
                    Style::default().fg(t.fg)
                };

                let mod_icon = if module.folded { "▸" } else { "▾" };
                let mod_status = if module.failed > 0 {
                    "✗".to_string()
                } else if module.running > 0 {
                    SPINNER[(frame_count as usize) % SPINNER.len()].to_string()
                } else {
                    "✔".to_string()
                };

                let mod_total = module.passed + module.failed + module.skipped + module.running;

                let mod_line = format!(
                    "    {mod_icon} [{mod_status}] {name}  {passed}/{mod_total}",
                    name = module.name,
                    passed = module.passed,
                    mod_total = mod_total,
                );
                lines.push(Line::from(Span::styled(mod_line, mod_style)));

                if !module.folded {
                    for (_, case) in module.cases.iter().enumerate() {
                        let case_color = match &case.status {
                            CaseStatus::Passed => t.accent,
                            CaseStatus::Failed(_) => t.error,
                            CaseStatus::Running => t.warn,
                            CaseStatus::Skipped => t.dim,
                            CaseStatus::Queued => t.dim,
                        };
                        let case_icon = match &case.status {
                            CaseStatus::Passed => "✔",
                            CaseStatus::Failed(_) => "✗",
                            CaseStatus::Running => "⏳",
                            CaseStatus::Skipped => "⏸",
                            CaseStatus::Queued => "·",
                        };
                        let case_line = format!(
                            "      [{case_icon}] {name}  {dur}ms",
                            name = case.name,
                            dur = case.duration_ms,
                        );
                        lines.push(Line::from(Span::styled(case_line, case_color)));

                        // Show failure output (first few lines).
                        if let CaseStatus::Failed(reason) = &case.status {
                            if !reason.is_empty() {
                                for line_text in reason.lines().take(MAX_OUTPUT_LINES) {
                                    lines.push(Line::from(Span::styled(
                                        format!("        {line_text}"),
                                        Style::default().fg(t.error).add_modifier(Modifier::DIM),
                                    )));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Window the list to fit the area.
    let h = area.height as usize;
    let total = lines.len();
    let end = total;
    let start = end.saturating_sub(h);
    let visible: Vec<Line> = if start < end {
        lines[start..end].to_vec()
    } else {
        Vec::new()
    };
    f.render_widget(Paragraph::new(visible).wrap(Wrap { trim: false }), area);
}

fn render_summary(f: &mut Frame, state: &VerifyState, t: &Theme, area: Rect) {
    let elapsed = if state.running {
        state.started_at.elapsed().as_secs_f64()
    } else {
        state.elapsed_ms as f64 / 1000.0
    };

    let status_text = if let Some(ref err) = state.error {
        format!(" 错误: {err}")
    } else if state.cancelled {
        " 已取消".to_string()
    } else {
        String::new()
    };

    let line = Line::from(vec![
        Span::styled(
            format!(" 通过:{}  失败:{}  跳过:{}  总计:{}  耗时:{:.1}s", state.passed, state.failed, state.skipped, state.total_tests, elapsed),
            if state.failed > 0 { Style::default().fg(t.error) } else { Style::default().fg(t.fg) },
        ),
        Span::styled(status_text, Style::default().fg(t.error)),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

fn render_shortcuts(f: &mut Frame, t: &Theme, area: Rect) {
    let line = Line::from(vec![
        Span::styled(" Tab 展开/折叠  ", Style::default().fg(t.dim)),
        Span::styled("↑↓ 选择  ", Style::default().fg(t.dim)),
        Span::styled("Enter 展开详情  ", Style::default().fg(t.dim)),
        Span::styled("Esc 退出  ", Style::default().fg(t.dim)),
        Span::styled("F5 重新运行  ", Style::default().fg(t.dim)),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

/// Handle verify-mode keyboard input. Returns whether the key was consumed.
pub fn handle_verify_key(state: &mut VerifyState, key: &crossterm::event::KeyEvent) -> bool {
    use crossterm::event::KeyCode;
    match key.code {
        KeyCode::Up => {
            match state.focus {
                VerifyFocus::None => {
                    // Start at the top of L1.
                    if !state.layers[0].modules.is_empty() {
                        state.focus = VerifyFocus::Case {
                            layer: 0,
                            module: 0,
                            case: 0,
                        };
                    } else {
                        state.focus = VerifyFocus::Layer(0);
                    }
                }
                VerifyFocus::Layer(l) => {
                    if l > 0 {
                        state.focus = VerifyFocus::Layer(l - 1);
                    }
                }
                VerifyFocus::Module { layer, module } => {
                    if module > 0 {
                        state.focus = VerifyFocus::Module { layer, module: module - 1 };
                    } else {
                        state.focus = VerifyFocus::Layer(layer);
                    }
                }
                VerifyFocus::Case { layer, module, case } => {
                    if case > 0 {
                        state.focus = VerifyFocus::Case { layer, module, case: case - 1 };
                    } else {
                        // Move up to module.
                        state.focus = VerifyFocus::Module { layer, module };
                    }
                }
            }
            true
        }
        KeyCode::Down => {
            match state.focus {
                VerifyFocus::None => {
                    if !state.layers[0].modules.is_empty() {
                        state.focus = VerifyFocus::Case { layer: 0, module: 0, case: 0 };
                    } else {
                        state.focus = VerifyFocus::Layer(0);
                    }
                }
                VerifyFocus::Layer(l) => {
                    if l < 2 {
                        // Move to first module if available.
                        if !state.layers[l].modules.is_empty() {
                            state.focus = VerifyFocus::Module { layer: l, module: 0 };
                        } else if l < 2 {
                            state.focus = VerifyFocus::Layer(l + 1);
                        }
                    }
                }
                VerifyFocus::Module { layer, module } => {
                    let next_module = module + 1;
                    if next_module < state.layers[layer].modules.len() {
                        state.focus = VerifyFocus::Module { layer, module: next_module };
                    } else if layer < 2 {
                        state.focus = VerifyFocus::Layer(layer + 1);
                    }
                }
                VerifyFocus::Case { layer, module, case } => {
                    let next_case = case + 1;
                    if next_case < state.layers[layer].modules[module].cases.len() {
                        state.focus = VerifyFocus::Case { layer, module, case: next_case };
                    } else {
                        // Move to next module.
                        let next_module = module + 1;
                        if next_module < state.layers[layer].modules.len() {
                            state.focus = VerifyFocus::Module { layer, module: next_module };
                        } else if layer < 2 {
                            state.focus = VerifyFocus::Layer(layer + 1);
                        }
                    }
                }
            }
            true
        }
        KeyCode::Tab | KeyCode::Enter => {
            // Toggle fold.
            match state.focus {
                VerifyFocus::Layer(l) => {
                    state.layers[l].folded = !state.layers[l].folded;
                }
                VerifyFocus::Module { layer, module } => {
                    if module < state.layers[layer].modules.len() {
                        state.layers[layer].modules[module].folded = !state.layers[layer].modules[module].folded;
                    }
                }
                VerifyFocus::Case { .. } => {
                    // Toggle case detail — handled by the TUI by expanding.
                }
                VerifyFocus::None => {}
            }
            true
        }
        KeyCode::F(5) => {
            // Reset for re-run (handled by caller).
            state.reset();
            true
        }
        KeyCode::Esc => {
            // Signal that Esc was consumed; caller will reset and cancel.
            true
        }
        _ => false,
    }
}
