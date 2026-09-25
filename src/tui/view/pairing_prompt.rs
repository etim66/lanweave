use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::app::interaction::{DialogFocus, PairingPromptInput};
use crate::app::model::AppModel;

use super::dialog::{self, Button};
use super::presenter::pairing_peer_label;
use super::theme::{ACCENT, MUTED, TEXT};

/// Renders the inbound pairing decision as a modal card with buttons.
///
/// The peer name is untrusted display text; the dialog never claims the device
/// is verified. The accepted choice is focused first and is activated with
/// Enter, while the arrow keys move to Reject.
pub(super) fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    model: &AppModel,
    prompt: &PairingPromptInput,
) {
    let peer = pairing_peer_label(model);
    let body = vec![
        Line::styled(format!("{peer} wants to pair."), Style::new().fg(TEXT)),
        Line::styled(
            "Only accept if the person is with you.",
            Style::new().fg(MUTED),
        ),
    ];
    let buttons = [
        Button::new("Accept", prompt.focus == DialogFocus::Accept),
        Button::new("Reject", prompt.focus == DialogFocus::Reject),
    ];
    let hints = Line::from(vec![
        Span::styled("left/right", Style::new().fg(TEXT)),
        Span::styled(" choose   ", Style::new().fg(MUTED)),
        Span::styled("enter", Style::new().fg(TEXT)),
        Span::styled(" select   ", Style::new().fg(MUTED)),
        Span::styled("esc", Style::new().fg(TEXT)),
        Span::styled(" reject", Style::new().fg(MUTED)),
    ]);

    dialog::render_dialog(
        frame,
        area,
        "Pairing request",
        ACCENT,
        body,
        &buttons,
        Some(hints),
    );
}
