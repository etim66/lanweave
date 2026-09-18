use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use crate::app::interaction::FileSelectionInput;
use crate::app::model::AppModel;
use crate::discovery::escape_display;

use super::layout::{centered_rect, inset_surface, surface_width};
use super::presenter::format_size;
use super::render_focus_rail;
use super::theme::{ACCENT, BACKGROUND, ERROR, HIGHLIGHT, MUTED, SURFACE, TEXT, WARNING};

/// Renders the local file review card: pending input, issues, files, and send.
///
/// Falls back to a single count line on very small terminals.
pub(super) fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    model: &AppModel,
    input: &FileSelectionInput,
) {
    if area.width < 12 || area.height < 5 {
        frame.render_widget(
            Paragraph::new(format!("{} file(s) reviewed", input.selection.len()))
                .alignment(Alignment::Center)
                .style(Style::new().fg(TEXT).add_modifier(Modifier::BOLD)),
            area,
        );
        return;
    }

    let stack = centered_rect(area, surface_width(area), 16);
    let show_hints = stack.height >= 6;
    let card_height = stack.height.saturating_sub(if show_hints { 2 } else { 0 });
    let card = Rect::new(stack.x, stack.y, stack.width, card_height);
    frame.render_widget(Block::new().style(Style::new().bg(SURFACE)), card);
    render_focus_rail(frame, card);

    let inner = inset_surface(card, 1);
    if inner.width > 0 && inner.height >= 2 {
        render_body(frame, inner, model, input);
    }

    if show_hints {
        let hints = Rect::new(
            stack.x,
            stack.y.saturating_add(card.height).saturating_add(1),
            stack.width,
            1,
        );
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("enter", Style::new().fg(TEXT)),
                Span::styled(" add / send   ", Style::new().fg(MUTED)),
                Span::styled("up/down", Style::new().fg(TEXT)),
                Span::styled(" select   ", Style::new().fg(MUTED)),
                Span::styled("backspace", Style::new().fg(TEXT)),
                Span::styled(" remove   ", Style::new().fg(MUTED)),
                Span::styled("esc", Style::new().fg(TEXT)),
                Span::styled(" close", Style::new().fg(MUTED)),
            ])),
            hints,
        );
    }
}

/// Renders the title, input line, issues, list, and status inside the card.
fn render_body(frame: &mut Frame<'_>, inner: Rect, model: &AppModel, input: &FileSelectionInput) {
    let status_y = inner.y + inner.height - 1;
    let title = format!(
        "Review files  ({} · {})",
        input.selection.len(),
        format_size(input.selection.total_size())
    );
    render_panel_line(
        frame,
        Rect::new(inner.x, inner.y, inner.width, 1),
        Line::styled(title, Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)),
    );

    let input_y = inner.y + 1;
    if input_y < status_y {
        render_panel_line(
            frame,
            Rect::new(inner.x, input_y, inner.width, 1),
            Line::from(vec![
                Span::styled("> ", Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)),
                Span::styled(input_display(&input.text), Style::new().fg(TEXT)),
            ]),
        );
    }

    let mut y = input_y.saturating_add(1);
    for issue in input.issues.iter().take(3) {
        if y >= status_y {
            break;
        }
        render_panel_line(
            frame,
            Rect::new(inner.x, y, inner.width, 1),
            Line::styled(
                format!("{}: {}", issue.path(), issue.reason()),
                Style::new().fg(ERROR),
            ),
        );
        y += 1;
    }

    if y < status_y {
        render_list(
            frame,
            Rect::new(inner.x, y, inner.width, status_y - y),
            input,
        );
    }

    render_panel_line(
        frame,
        Rect::new(inner.x, status_y, inner.width, 1),
        status_line(model, input),
    );
}

/// Renders the reviewed files, scrolled around the highlighted entry.
fn render_list(frame: &mut Frame<'_>, area: Rect, input: &FileSelectionInput) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let files = input.selection.files();
    if files.is_empty() {
        render_panel_line(
            frame,
            area,
            Line::styled(
                "No files reviewed yet. Paste paths and press enter.",
                Style::new().fg(MUTED),
            ),
        );
        return;
    }

    let capacity = usize::from(area.height);
    let selected = input.selected.unwrap_or(0);
    let start = selected.saturating_add(1).saturating_sub(capacity);

    for (offset, file) in files.iter().skip(start).take(capacity).enumerate() {
        let index = start + offset;
        let highlighted = input.selected == Some(index);
        let row_style = if highlighted {
            Style::new().bg(HIGHLIGHT).fg(BACKGROUND)
        } else {
            Style::new().bg(SURFACE).fg(TEXT)
        };
        let line = Line::from(vec![
            Span::styled(
                format!("  {} ", index + 1),
                row_style.add_modifier(Modifier::BOLD),
            ),
            Span::styled(escape_display(file.name()), row_style),
            Span::styled(format!("  {}", format_size(file.size())), row_style),
        ]);
        frame.render_widget(
            Paragraph::new(line).style(row_style),
            Rect::new(
                area.x,
                area.y + u16::try_from(offset).unwrap_or(u16::MAX),
                area.width,
                1,
            ),
        );
    }
}

/// Describes whether the reviewed files can be sent right now.
fn status_line(model: &AppModel, input: &FileSelectionInput) -> Line<'static> {
    if input.selection.is_empty() {
        return Line::styled(
            "Paste or type file paths, then press enter.",
            Style::new().fg(MUTED),
        );
    }
    if model.capabilities().can_start_transfer {
        return Line::styled(
            "Press enter to send the reviewed files.",
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        );
    }
    Line::styled(
        "Connect and pair an authorized session before sending.",
        Style::new().fg(WARNING),
    )
}

/// Renders the pending input as one line.
fn input_display(text: &str) -> String {
    if text.is_empty() {
        return "Paste file paths, one per line".to_owned();
    }
    if text.contains('\n') {
        return format!(
            "{} lines ready; press enter to review",
            text.lines().count()
        );
    }
    escape_display(text)
}

/// Renders one background-filled line inside the review card.
fn render_panel_line(frame: &mut Frame<'_>, area: Rect, line: Line<'_>) {
    frame.render_widget(Paragraph::new(line).style(Style::new().bg(SURFACE)), area);
}
