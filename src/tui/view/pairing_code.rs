use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use crate::app::interaction::PairingCodeInput;
use crate::app::model::AppModel;

use super::layout::{centered_rect, inset_surface, surface_width};
use super::presenter::status_text;
use super::render_focus_rail;
use super::theme::{ACCENT, MUTED, SURFACE, TEXT};

/// Renders the initiator's pairing-code entry card.
///
/// The entered digits are shown grouped and in the clear because the local
/// user is reading them from the other device; the code still never appears
/// in logs, errors, or `Debug` output. Falls back to the raw input line on
/// very small terminals.
pub(super) fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    model: &AppModel,
    input: &PairingCodeInput,
) {
    if area.width < 8 || area.height < 3 {
        frame.render_widget(
            Paragraph::new(input.grouped())
                .alignment(Alignment::Center)
                .style(Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)),
            area,
        );
        return;
    }

    let stack = centered_rect(area, surface_width(area), 6);
    let card = Rect::new(stack.x, stack.y, stack.width, 4);
    frame.render_widget(Block::new().style(Style::new().bg(SURFACE)), card);
    render_focus_rail(frame, card);
    let inner = inset_surface(card, 1);

    render_panel_line(
        frame,
        Rect::new(inner.x, inner.y, inner.width, 1),
        Line::styled(
            "Enter the pairing code",
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
    );

    render_panel_line(
        frame,
        Rect::new(inner.x, inner.y + 1, inner.width, 1),
        Line::from(vec![
            Span::styled("> ", Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled(input.grouped(), Style::new().fg(TEXT)),
        ]),
    );

    render_panel_line(
        frame,
        Rect::new(inner.x, inner.y + 2, inner.width, 1),
        Line::styled(
            "The code is shown on the other device and expires after about two minutes.",
            Style::new().fg(MUTED),
        ),
    );

    let hints = Rect::new(
        stack.x,
        stack.y.saturating_add(card.height).saturating_add(1),
        stack.width,
        1,
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("0-9", Style::new().fg(TEXT)),
            Span::styled(" type   ", Style::new().fg(MUTED)),
            Span::styled("backspace", Style::new().fg(TEXT)),
            Span::styled(" delete   ", Style::new().fg(MUTED)),
            Span::styled("enter", Style::new().fg(TEXT)),
            Span::styled(" submit   ", Style::new().fg(MUTED)),
            Span::styled("esc", Style::new().fg(TEXT)),
            Span::styled(" cancel", Style::new().fg(MUTED)),
        ])),
        hints,
    );

    let status = Rect::new(
        stack.x,
        stack.y.saturating_add(card.height).saturating_add(2),
        stack.width,
        1,
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Lanweave", Style::new().fg(ACCENT)),
            Span::styled("  /  ", Style::new().fg(MUTED)),
            Span::styled(status_text(model.state()), Style::new().fg(MUTED)),
        ])),
        status,
    );
}

/// Renders one background-filled line inside the input card.
fn render_panel_line(frame: &mut Frame<'_>, area: Rect, line: Line<'_>) {
    frame.render_widget(Paragraph::new(line).style(Style::new().bg(SURFACE)), area);
}
