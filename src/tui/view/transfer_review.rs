use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use crate::app::interaction::TransferReviewInput;
use crate::app::model::AppModel;
use crate::discovery::escape_display;

use super::layout::{centered_rect, inset_surface, surface_width};
use super::presenter::format_size;
use super::render_focus_rail;
use super::theme::{ACCENT, ERROR, MUTED, SURFACE, TEXT, WARNING};

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
                Span::styled("enter", Style::new().fg(TEXT)),
                Span::styled(" accept   ", Style::new().fg(MUTED)),
                Span::styled("esc", Style::new().fg(TEXT)),
                Span::styled(" reject", Style::new().fg(MUTED)),
            ])),
            hints,
        );
    }
}

/// Renders the title, peer, manifest, destination, and any error.
fn render_body(frame: &mut Frame<'_>, inner: Rect, model: &AppModel, input: &TransferReviewInput) {
    let mut y = inner.y;

    // Reserve the destination row and, when present, the error row above it.
    let footer = 1 + u16::from(input.error.is_some());
    let list_end = (inner.y + inner.height).saturating_sub(footer);
    let destination_y = inner.y + inner.height - 1;

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
    let capacity = list_end.saturating_sub(y);
    if let Some(proposal) = model.transfer_proposal() {
        let shown = usize::from(capacity).min(proposal.files().len());
        for (index, entry) in proposal.files().iter().take(shown).enumerate() {
            let mut spans = vec![
                Span::styled(
                    format!("  {} ", index + 1),
                    Style::new().fg(TEXT).add_modifier(Modifier::BOLD),
                ),
                Span::styled(escape_display(&entry.name), Style::new().fg(TEXT)),
                Span::styled(
                    format!("  {}", format_size(entry.size)),
                    Style::new().fg(MUTED),
                ),
            ];
            if let Some(folder) = &entry.folder {
                spans.push(Span::styled(
                    format!(
                        "  folder · {} items · {}",
                        folder.items,
                        format_size(folder.source_size)
                    ),
                    Style::new().fg(WARNING),
                ));
            }
            let line = Line::from(spans);
            render_panel_line(frame, Rect::new(inner.x, y, inner.width, 1), line);
            y += 1;
        }
        if shown < proposal.files().len() && y < list_end {
            render_panel_line(
                frame,
                Rect::new(inner.x, y, inner.width, 1),
                Line::styled(
                    format!("  … and {} more", proposal.files().len() - shown),
                    Style::new().fg(MUTED),
                ),
            );
        }
    }

    if let Some(error) = input.error {
        render_panel_line(
            frame,
            Rect::new(inner.x, list_end, inner.width, 1),
            Line::styled(error, Style::new().fg(ERROR)),
        );
    }

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

/// Renders one background-filled line inside the review card.
fn render_panel_line(frame: &mut Frame<'_>, area: Rect, line: Line<'_>) {
    frame.render_widget(Paragraph::new(line).style(Style::new().bg(SURFACE)), area);
}
