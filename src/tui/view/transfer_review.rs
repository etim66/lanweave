use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::interaction::{DialogFocus, TransferReviewInput};
use crate::app::model::AppModel;
use crate::discovery::escape_display;

use super::dialog::{self, Button};
use super::layout::scroll_window;
use super::presenter::format_size;
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

    let title = match model.transfer_proposal() {
        Some(proposal) => format!(
            "Incoming files  ({} · {})",
            proposal.files().len(),
            format_size(proposal.total_size())
        ),
        None => "Incoming files".to_owned(),
    };
    let hints = Line::from(vec![
        Span::styled("left/right", Style::new().fg(TEXT)),
        Span::styled(" choose   ", Style::new().fg(MUTED)),
        Span::styled("up/down", Style::new().fg(TEXT)),
        Span::styled(" scroll", Style::new().fg(MUTED)),
    ]);
    let buttons = [
        Button::new("Accept", input.focus == DialogFocus::Accept),
        Button::new("Reject", input.focus == DialogFocus::Reject),
    ];

    let spec = dialog::DialogSpec::new(&title, ACCENT)
        .header_hint("esc")
        .buttons(&buttons)
        .footer_hint(hints);
    dialog::render_dialog_with(frame, area, spec, 9, |frame, body| {
        render_body(frame, body, model, input);
    });
}

/// Renders the peer note, manifest, destination, and error inside the body.
fn render_body(frame: &mut Frame<'_>, body: Rect, model: &AppModel, input: &TransferReviewInput) {
    if body.width == 0 || body.height == 0 {
        return;
    }

    let last_y = body.y + body.height - 1;
    let (error_y, destination_y) = match (input.error.is_some(), body.height) {
        // A one-row body still shows the error, which needs action, rather
        // than spending its only row on the destination.
        (true, 1) => (Some(body.y), None),
        (true, _) => (Some(last_y.saturating_sub(1)), Some(last_y)),
        (false, _) => (None, Some(last_y)),
    };
    let list_bottom = error_y
        .or(destination_y)
        .unwrap_or(body.y)
        .saturating_sub(1);
    let peer_y = body.y;
    let list_y = peer_y.saturating_add(1);

    if peer_y < list_bottom {
        let peer = model
            .transfer_proposal()
            .and_then(|proposal| proposal.peer())
            .map(|name| format!("From {name} — the name is untrusted"))
            .unwrap_or_else(|| "From the connected device — unverified".to_owned());
        render_panel_line(
            frame,
            Rect::new(body.x, peer_y, body.width, 1),
            Line::styled(peer, Style::new().fg(WARNING)),
        );
    }

    if let Some(proposal) = model.transfer_proposal() {
        let files = proposal.files();
        let capacity = usize::from(list_bottom.saturating_sub(list_y));
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
                Rect::new(
                    body.x,
                    list_y.saturating_add(u16::try_from(offset).unwrap_or(u16::MAX)),
                    body.width,
                    1,
                ),
            );
        }
    }

    if let (Some(error), Some(error_y)) = (input.error, error_y) {
        render_panel_line(
            frame,
            Rect::new(body.x, error_y, body.width, 1),
            Line::styled(error, Style::new().fg(ERROR)),
        );
    }

    if let Some(destination_y) = destination_y {
        render_panel_line(
            frame,
            Rect::new(body.x, destination_y, body.width, 1),
            Line::from(vec![
                Span::styled(
                    "Save to: ",
                    Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
                ),
                Span::styled(escape_display(&input.destination), Style::new().fg(TEXT)),
            ]),
        );
    }
}

/// Renders one background-filled line inside the review body.
fn render_panel_line(frame: &mut Frame<'_>, area: Rect, line: Line<'_>) {
    frame.render_widget(Paragraph::new(line).style(Style::new().bg(SURFACE)), area);
}
