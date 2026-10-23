use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::app::interaction::DirectAddressInput;
use crate::app::model::AppModel;

use super::dialog;
use super::theme::{ACCENT, ERROR, MUTED, TEXT};

/// Renders the direct-address input dialog with live validation feedback.
pub(super) fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    _model: &AppModel,
    input: &DirectAddressInput,
) {
    let (feedback, color) = match input.error {
        Some(error) => (error.message().to_owned(), ERROR),
        None => ("example: peer.local:4242".to_owned(), MUTED),
    };
    let body = vec![
        Line::from(vec![
            Span::styled("> ", Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled(input.text.clone(), Style::new().fg(TEXT)),
        ]),
        Line::default(),
        Line::styled(feedback, Style::new().fg(color)),
    ];
    let hints = Line::from(vec![
        Span::styled("enter", Style::new().fg(TEXT)),
        Span::styled(" connect   ", Style::new().fg(MUTED)),
        Span::styled("esc", Style::new().fg(TEXT)),
        Span::styled(" cancel", Style::new().fg(MUTED)),
    ]);

    let spec = dialog::DialogSpec::new("Connect to a host:port", ACCENT)
        .header_hint("esc")
        .footer_hint(hints);
    dialog::render_dialog(frame, area, spec, body);
}
