//! LazyAlign - UI rendering
//!
//! Renders the main interface using Ratatui widgets.

use crate::app::{App, ViewMode};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
    Frame,
};

/// Theme colors for the UI
pub struct Theme {
    pub bg: Color,
    pub fg: Color,
    pub accent: Color,
    pub error: Color,
    pub warning: Color,
    pub border: Color,
    pub highlight: Color,
    pub muted: Color,
}

impl Default for Theme {
    fn default() -> Self {
        // Dracula-inspired dark theme
        Self {
            bg: Color::Rgb(40, 42, 54),
            fg: Color::Rgb(248, 248, 242),
            accent: Color::Rgb(139, 233, 253),
            error: Color::Rgb(255, 85, 85),
            warning: Color::Rgb(255, 184, 108),
            border: Color::Rgb(98, 114, 164),
            highlight: Color::Rgb(68, 71, 90),
            muted: Color::Rgb(98, 114, 164),
        }
    }
}

/// Render the entire UI
pub fn render(frame: &mut Frame, app: &mut App) {
    let theme = Theme::default();

    // Update viewport height based on frame size
    app.set_viewport_height(frame.area().height as usize);

    // Main layout: content area + status bar
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(3)])
        .split(frame.area());

    // Render main content area
    render_content(frame, app, chunks[0], &theme);

    // Render status bar
    render_status_bar(frame, app, chunks[1], &theme);

    // Render help popup if visible
    if app.show_help {
        render_help_popup(frame, &theme);
    }
}

/// Render the main content area with dataset lines
fn render_content(frame: &mut Frame, app: &App, area: Rect, theme: &Theme) {
    let visible_lines = (area.height as usize).saturating_sub(2);

    let items: Vec<ListItem> = (0..visible_lines)
        .filter_map(|i| {
            let line_idx = app.scroll + i;
            let line_content = app.dataset.get_line(line_idx)?;

            // Truncate long lines for display
            let display_width = area.width as usize - 10;
            let truncated = if line_content.len() > display_width {
                format!("{}...", &line_content[..display_width.saturating_sub(3)])
            } else {
                line_content.to_string()
            };

            // Create styled line based on view mode and lint status
            let line: Line = if app.line_has_error(line_idx) {
                // Error line - highlight in red
                Line::from(vec![
                    Span::styled(
                        format!("{:>6} │ ", line_idx + 1),
                        Style::default().fg(theme.error),
                    ),
                    Span::styled(
                        truncated,
                        Style::default().fg(theme.error).add_modifier(Modifier::BOLD),
                    ),
                ])
            } else if app.view_mode == ViewMode::TokenXray {
                // Token X-Ray mode
                if let Some(ref tokenizer) = app.tokenizer {
                    let token_line = tokenizer.colorize_tokens(&truncated);
                    let mut spans = vec![Span::styled(
                        format!("{:>6} │ ", line_idx + 1),
                        Style::default().fg(theme.muted),
                    )];
                    spans.extend(token_line.spans);
                    Line::from(spans)
                } else {
                    Line::from(vec![
                        Span::styled(
                            format!("{:>6} │ ", line_idx + 1),
                            Style::default().fg(theme.muted),
                        ),
                        Span::styled(truncated, Style::default().fg(theme.fg)),
                    ])
                }
            } else {
                // Normal text mode with JSON syntax highlighting
                let highlighted = highlight_json(&truncated, theme);
                let mut spans = vec![Span::styled(
                    format!("{:>6} │ ", line_idx + 1),
                    Style::default().fg(theme.muted),
                )];
                spans.extend(highlighted.spans);
                Line::from(spans)
            };

            // Highlight selected line
            let style = if line_idx == app.selected_line {
                Style::default().bg(theme.highlight)
            } else {
                Style::default()
            };

            Some(ListItem::new(line).style(style))
        })
        .collect();

    let mode_indicator = format!(" {} ", app.view_mode.label());
    let title = format!(
        " LazyAlign │ {} │ {} lines │ {} ",
        app.dataset.path.split('/').last().unwrap_or("file"),
        app.dataset.line_count(),
        mode_indicator
    );

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.border))
            .title(Span::styled(title, Style::default().fg(theme.accent)))
            .style(Style::default().bg(theme.bg)),
    );

    frame.render_widget(list, area);
}

/// Render the status bar
fn render_status_bar(frame: &mut Frame, app: &App, area: Rect, theme: &Theme) {
    let lint_count = app.lint_results.len();
    let lint_status = if lint_count > 0 {
        format!(" ⚠ {} issues ", lint_count)
    } else {
        " ✓ No issues ".to_string()
    };

    let lint_style = if lint_count > 0 {
        Style::default().fg(theme.warning)
    } else {
        Style::default().fg(Color::Rgb(80, 250, 123)) // Green
    };

    let tokenizer_status = if let Some(ref t) = app.tokenizer {
        format!(" 🔤 {} ", t.name)
    } else {
        " No tokenizer ".to_string()
    };

    let position = format!(
        " Line {}/{} ",
        app.selected_line + 1,
        app.dataset.line_count()
    );

    let status_line = Line::from(vec![
        Span::styled(" LazyAlign v0.1.0 ", Style::default().fg(theme.accent).add_modifier(Modifier::BOLD)),
        Span::styled("│", Style::default().fg(theme.border)),
        Span::styled(format!(" {} ", app.dataset.size_human()), Style::default().fg(theme.fg)),
        Span::styled("│", Style::default().fg(theme.border)),
        Span::styled(lint_status, lint_style),
        Span::styled("│", Style::default().fg(theme.border)),
        Span::styled(tokenizer_status, Style::default().fg(theme.muted)),
        Span::styled("│", Style::default().fg(theme.border)),
        Span::styled(position, Style::default().fg(theme.fg)),
        Span::styled("│", Style::default().fg(theme.border)),
        Span::styled(" ?:Help q:Quit ", Style::default().fg(theme.muted)),
    ]);

    let status_bar = Paragraph::new(status_line).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.border))
            .style(Style::default().bg(theme.bg)),
    );

    frame.render_widget(status_bar, area);
}

/// Render help popup
fn render_help_popup(frame: &mut Frame, theme: &Theme) {
    let area = centered_rect(50, 60, frame.area());

    // Clear the background
    frame.render_widget(Clear, area);

    let help_text = vec![
        Line::from(Span::styled("⌨️  Keyboard Shortcuts", Style::default().fg(theme.accent).add_modifier(Modifier::BOLD))),
        Line::from(""),
        Line::from(vec![
            Span::styled("  j / ↓    ", Style::default().fg(theme.warning)),
            Span::raw("Move down"),
        ]),
        Line::from(vec![
            Span::styled("  k / ↑    ", Style::default().fg(theme.warning)),
            Span::raw("Move up"),
        ]),
        Line::from(vec![
            Span::styled("  g        ", Style::default().fg(theme.warning)),
            Span::raw("Go to top"),
        ]),
        Line::from(vec![
            Span::styled("  G        ", Style::default().fg(theme.warning)),
            Span::raw("Go to bottom"),
        ]),
        Line::from(vec![
            Span::styled("  Ctrl+d   ", Style::default().fg(theme.warning)),
            Span::raw("Page down"),
        ]),
        Line::from(vec![
            Span::styled("  Ctrl+u   ", Style::default().fg(theme.warning)),
            Span::raw("Page up"),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Tab      ", Style::default().fg(theme.accent)),
            Span::raw("Toggle Token X-Ray mode"),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  ?        ", Style::default().fg(theme.muted)),
            Span::raw("Toggle this help"),
        ]),
        Line::from(vec![
            Span::styled("  q        ", Style::default().fg(theme.error)),
            Span::raw("Quit"),
        ]),
    ];

    let help = Paragraph::new(help_text)
        .block(
            Block::default()
                .title(Span::styled(" Help ", Style::default().fg(theme.accent)))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.border))
                .style(Style::default().bg(theme.bg)),
        )
        .wrap(Wrap { trim: false });

    frame.render_widget(help, area);
}

/// Basic JSON syntax highlighting
fn highlight_json(text: &str, theme: &Theme) -> Line<'static> {
    let mut spans = Vec::new();
    let mut chars = text.chars().peekable();
    let mut current = String::new();
    let mut is_key = true;

    while let Some(c) = chars.next() {
        match c {
            '"' => {
                if !current.is_empty() {
                    spans.push(Span::raw(current.clone()));
                    current.clear();
                }

                // Find the end of the string
                let mut string_content = String::from('"');
                let mut escaped = false;
                for ch in chars.by_ref() {
                    string_content.push(ch);
                    if ch == '"' && !escaped {
                        break;
                    }
                    escaped = ch == '\\' && !escaped;
                }

                let color = if is_key {
                    theme.accent
                } else {
                    Color::Rgb(241, 250, 140) // Yellow for values
                };
                spans.push(Span::styled(string_content, Style::default().fg(color)));
            }
            ':' => {
                if !current.is_empty() {
                    spans.push(Span::raw(current.clone()));
                    current.clear();
                }
                spans.push(Span::styled(":", Style::default().fg(theme.fg)));
                is_key = false;
            }
            ',' => {
                if !current.is_empty() {
                    spans.push(Span::raw(current.clone()));
                    current.clear();
                }
                spans.push(Span::styled(",", Style::default().fg(theme.fg)));
                is_key = true;
            }
            '{' | '}' | '[' | ']' => {
                if !current.is_empty() {
                    spans.push(Span::raw(current.clone()));
                    current.clear();
                }
                spans.push(Span::styled(
                    c.to_string(),
                    Style::default().fg(theme.warning),
                ));
                if c == '{' || c == '[' {
                    is_key = c == '{';
                }
            }
            _ => {
                current.push(c);
            }
        }
    }

    if !current.is_empty() {
        // Check if it's a number or boolean
        let trimmed = current.trim();
        let color = if trimmed.parse::<f64>().is_ok() {
            Color::Rgb(189, 147, 249) // Purple for numbers
        } else if trimmed == "true" || trimmed == "false" || trimmed == "null" {
            Color::Rgb(255, 121, 198) // Pink for booleans
        } else {
            theme.fg
        };
        spans.push(Span::styled(current, Style::default().fg(color)));
    }

    Line::from(spans)
}

/// Helper to create a centered rect
fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}
