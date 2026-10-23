use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::interaction::PairingCodeInput;
use crate::app::model::AppModel;

use super::dialog;
use super::digits;
use super::theme::{ACCENT, MUTED, SURFACE, TEXT};

/// Renders the initiator's pairing-code entry dialog.
///
/// The entered digits are shown grouped and in the clear because the local
/// user is reading them from the other device; the code still never appears
/// in logs, errors, or `Debug` output.
pub(super) fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    _model: &AppModel,
    input: &PairingCodeInput,
) {
    if area.width < 12 || area.height < 5 {
        frame.render_widget(
            Paragraph::new(input.grouped())
                .alignment(Alignment::Center)
                .style(Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)),
            area,
        );
        return;
    }

    let hints = Line::from(vec![
        Span::styled("0-9", Style::new().fg(TEXT)),
        Span::styled(" type   ", Style::new().fg(MUTED)),
        Span::styled("backspace", Style::new().fg(TEXT)),
        Span::styled(" delete   ", Style::new().fg(MUTED)),
        Span::styled("enter", Style::new().fg(TEXT)),
        Span::styled(" submit", Style::new().fg(MUTED)),
    ]);

    let spec = dialog::DialogSpec::new("Enter the pairing code", ACCENT)
        .header_hint("esc")
        .footer_hint(hints);
    dialog::render_dialog_with(frame, area, spec, 7, |frame, body| {
        render_body(frame, body, input);
    });
}

/// Renders the large digit display and its expiry note inside the dialog body.
fn render_body(frame: &mut Frame<'_>, body: Rect, input: &PairingCodeInput) {
    if body.width == 0 || body.height == 0 {
        return;
    }

    let digits: Vec<char> = input.grouped().chars().collect();
    if body.height < 5 {
        let text = if digits.is_empty() {
            "····   ····".to_owned()
        } else {
            input.grouped()
        };
        frame.render_widget(
            Paragraph::new(Line::styled(
                text,
                Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
            ))
            .alignment(Alignment::Center)
            .style(Style::new().bg(SURFACE)),
            Rect::new(body.x, body.y, body.width, 1),
        );
        return;
    }
    if digits.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::styled(
                "····   ····",
                Style::new().fg(MUTED).add_modifier(Modifier::BOLD),
            ))
            .alignment(Alignment::Center)
            .style(Style::new().bg(SURFACE)),
            Rect::new(
                body.x,
                body.y.saturating_add(2).min(body.y + body.height - 1),
                body.width,
                1,
            ),
        );
    } else {
        let glyph_width = u16::try_from(digits::width(digits.len())).unwrap_or(u16::MAX);
        if glyph_width > body.width {
            frame.render_widget(
                Paragraph::new(Line::styled(
                    input.grouped(),
                    Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
                ))
                .alignment(Alignment::Center)
                .style(Style::new().bg(SURFACE)),
                Rect::new(
                    body.x,
                    body.y.saturating_add(2).min(body.y + body.height - 1),
                    body.width,
                    1,
                ),
            );
        } else {
            for row_index in 0..digits::ROWS {
                let row_y = body
                    .y
                    .saturating_add(1 + u16::try_from(row_index).unwrap_or(u16::MAX));
                if row_y >= body.y + body.height {
                    break;
                }
                frame.render_widget(
                    Paragraph::new(Line::styled(
                        digits::row(&digits, row_index),
                        Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
                    ))
                    .alignment(Alignment::Center)
                    .style(Style::new().bg(SURFACE)),
                    Rect::new(body.x, row_y, body.width, 1),
                );
            }
        }
    }

    if body.height >= 7 {
        frame.render_widget(
            Paragraph::new(Line::styled(
                "The other device shows this code; it expires after about two minutes.",
                Style::new().fg(MUTED),
            ))
            .style(Style::new().bg(SURFACE)),
            Rect::new(body.x, body.y + body.height - 1, body.width, 1),
        );
    }
}
