use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use crate::app::interaction::{DialogFocus, TransferReviewInput};
use crate::app::model::AppModel;
use crate::discovery::escape_display;

use super::dialog::{self, Button};
use super::layout::{centered_rect, inset_surface, scroll_window, surface_width};
use super::presenter::format_size;
use super::render_focus_rail;
use super::theme::{ACCENT, BACKGROUND, ERROR, HIGHLIGHT, MUTED, SURFACE, TEXT, WARNING};

/// Renders the inbound transfer review: manifest, destination, and decision.
///
/// The peer name is untrusted and the manifest comes only from the validated
/// `transfer_request`; both are escaped before display.
pub(super) fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    model: &AppModel,
    input: &TransferReviewInput,
) {
    let count = model
        .transfer_proposal()
        .map(|proposal| proposal.files().len())
        .unwrap_or(0);
    if area.width < 12 || area.height < 5 {
        frame.render_widget(
            Paragraph::new(format!("{count} incoming file(s)"))
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
                Span::styled("left/right", Style::new().fg(TEXT)),
                Span::styled(" choose   ", Style::new().fg(MUTED)),
                Span::styled("enter", Style::new().fg(TEXT)),
                Span::styled(" select   ", Style::new().fg(MUTED)),
                Span::styled("up/down", Style::new().fg(TEXT)),
                Span::styled(" scroll   ", Style::new().fg(MUTED)),
                Span::styled("esc", Style::new().fg(TEXT)),
                Span::styled(" reject", Style::new().fg(MUTED)),
            ])),
            hints,
        );
    }
}

/// Renders the title, peer, manifest, destination, error, and decision buttons.
fn render_body(frame: &mut Frame<'_>, inner: Rect, model: &AppModel, input: &TransferReviewInput) {
    let mut y = inner.y;
    let bottom = inner.y + inner.height;
    let buttons_y = bottom.saturating_sub(1);
    // An error replaces the destination line when the card is short, but keeps
    // its own row above the destination when there is room.
    let error_rows = u16::from(input.error.is_some());
    let (error_y, destination_y) = match (error_rows, inner.height) {
        (1, height) if height >= 4 => (Some(bottom - 2), Some(bottom - 3)),
        (1, _) => (Some(bottom.saturating_sub(2)), None),
        (_, height) if height >= 2 => (None, Some(bottom - 2)),
        _ => (None, None),
    };
    let list_end = error_y
        .or(destination_y)
        .unwrap_or(buttons_y)
        .min(buttons_y);

    let title = match model.transfer_proposal() {
        Some(proposal) => format!(
            "Incoming files  ({} · {})",
            proposal.files().len(),
            format_size(proposal.total_size())
        ),
        None => "Incoming files".to_owned(),
    };
    render_panel_line(
        frame,
        Rect::new(inner.x, y, inner.width, 1),
        Line::styled(title, Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)),
    );
    y += 1;

    if y < list_end {
        let peer = model
            .transfer_proposal()
            .and_then(|proposal| proposal.peer())
            .map(|name| format!("From {name} — the name is untrusted"))
            .unwrap_or_else(|| "From the connected device — unverified".to_owned());
        render_panel_line(
            frame,
            Rect::new(inner.x, y, inner.width, 1),
            Line::styled(peer, Style::new().fg(WARNING)),
        );
        y += 1;
    }

    // The footer rows are reserved even when nothing else fits.
    let capacity = usize::from(list_end.saturating_sub(y));
    if let Some(proposal) = model.transfer_proposal() {
        let files = proposal.files();
        let (start, cursor) = scroll_window(files.len(), input.scroll, capacity);
        for (offset, entry) in files.iter().skip(start).take(capacity).enumerate() {
            let index = start + offset;
            let highlighted = index == cursor;
            let row_style = if highlighted {
                Style::new().bg(HIGHLIGHT).fg(BACKGROUND)
            } else {
                Style::new().bg(SURFACE).fg(TEXT)
            };
            let muted_style = if highlighted {
                row_style
            } else {
                Style::new().fg(MUTED)
            };
            let mut spans = vec![
                Span::styled(
                    format!("  {} ", index + 1),
                    row_style.add_modifier(Modifier::BOLD),
                ),
                Span::styled(escape_display(&entry.name), row_style),
                Span::styled(format!("  {}", format_size(entry.size)), muted_style),
            ];
            if let Some(folder) = &entry.folder {
                spans.push(Span::styled(
                    format!(
                        "  folder · {} items · {}",
                        folder.items,
                        format_size(folder.source_size)
                    ),
                    if highlighted {
                        row_style
                    } else {
                        Style::new().fg(WARNING)
                    },
                ));
            }
            frame.render_widget(
                Paragraph::new(Line::from(spans)).style(row_style),
                Rect::new(inner.x, y, inner.width, 1),
            );
            y += 1;
        }
    }

    if let (Some(error), Some(error_y)) = (input.error, error_y) {
        render_panel_line(
            frame,
            Rect::new(inner.x, error_y, inner.width, 1),
            Line::styled(error, Style::new().fg(ERROR)),
        );
    }

    if let Some(destination_y) = destination_y {
        render_panel_line(
            frame,
            Rect::new(inner.x, destination_y, inner.width, 1),
            Line::from(vec![
                Span::styled(
                    "Save to: ",
                    Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
                ),
                Span::styled(escape_display(&input.destination), Style::new().fg(TEXT)),
            ]),
        );
    }

    let buttons = [
        Button::new("Accept", input.focus == DialogFocus::Accept),
        Button::new("Reject", input.focus == DialogFocus::Reject),
    ];
    dialog::render_buttons(
        frame,
        Rect::new(inner.x, buttons_y, inner.width, 1),
        &buttons,
    );
}

/// Renders one background-filled line inside the review card.
fn render_panel_line(frame: &mut Frame<'_>, area: Rect, line: Line<'_>) {
    frame.render_widget(Paragraph::new(line).style(Style::new().bg(SURFACE)), area);
}
