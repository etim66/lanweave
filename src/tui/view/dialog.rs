use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use super::layout::{centered_rect, inset_surface, surface_width};
use super::render_focus_rail;
use super::theme::{ACCENT, BACKGROUND, HIGHLIGHT, MUTED, SURFACE, TEXT};

/// One selectable action in a dialog or action bar.
///
/// The focused button is the one Enter activates; the user moves between
/// buttons with the arrow keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Button {
    pub(super) label: &'static str,
    pub(super) focused: bool,
}

impl Button {
    /// Builds a button, focused or not.
    pub(super) const fn new(label: &'static str, focused: bool) -> Self {
        Self { label, focused }
    }
}

/// Renders a centered row of action buttons, highlighting the focused one.
pub(super) fn render_buttons(frame: &mut Frame<'_>, area: Rect, buttons: &[Button]) {
    if area.width == 0 || area.height == 0 || buttons.is_empty() {
        return;
    }
    frame.render_widget(
        Paragraph::new(buttons_line(buttons))
            .alignment(Alignment::Center)
            .style(Style::new().bg(SURFACE)),
        area,
    );
}

/// Builds the centered button row: the focused button is accent-filled while
/// the others stay bracketed and muted.
pub(super) fn buttons_line(buttons: &[Button]) -> Line<'static> {
    let mut spans = Vec::with_capacity(buttons.len() * 2);
    for (index, button) in buttons.iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw("  "));
        }
        if button.focused {
            spans.push(Span::styled(
                format!(" {} ", button.label),
                Style::new()
                    .bg(HIGHLIGHT)
                    .fg(BACKGROUND)
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            spans.push(Span::styled(
                format!("[ {} ]", button.label),
                Style::new().fg(MUTED),
            ));
        }
    }
    Line::from(spans)
}

/// Renders a centered modal card with a title, body, buttons, and hints.
///
/// The card is sized to its content and clamped to `area`. The focused button
/// sits on the last card row; `hints` are rendered below the card when the
/// terminal is tall enough.
pub(super) fn render_dialog(
    frame: &mut Frame<'_>,
    area: Rect,
    title: &str,
    title_color: Color,
    body: Vec<Line<'static>>,
    buttons: &[Button],
    hints: Option<Line<'static>>,
) {
    if area.width < 12 || area.height < 5 {
        render_compact(frame, area, title, &body, buttons);
        return;
    }

    let body_rows = u16::try_from(body.len()).unwrap_or(u16::MAX);
    // Title, a blank row, the body, a blank row, and the button row.
    let desired = body_rows.saturating_add(4).max(6);
    let hint_rows = if hints.is_some() { 2 } else { 0 };
    let stack = centered_rect(area, surface_width(area), desired.saturating_add(hint_rows));
    let card = Rect::new(
        stack.x,
        stack.y,
        stack.width,
        stack.height.saturating_sub(hint_rows),
    );
    frame.render_widget(Block::new().style(Style::new().bg(SURFACE)), card);
    render_focus_rail(frame, card);

    let inner = inset_surface(card, 1);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    render_panel_line(
        frame,
        Rect::new(inner.x, inner.y, inner.width, 1),
        Line::styled(
            title,
            Style::new().fg(title_color).add_modifier(Modifier::BOLD),
        ),
    );

    let button_y = inner.y + inner.height - 1;
    for (y, line) in (inner.y.saturating_add(2)..button_y).zip(body) {
        render_panel_line(frame, Rect::new(inner.x, y, inner.width, 1), line);
    }
    render_buttons(frame, Rect::new(inner.x, button_y, inner.width, 1), buttons);

    if let Some(hints) = hints {
        frame.render_widget(
            Paragraph::new(hints),
            Rect::new(
                stack.x,
                stack.y.saturating_add(card.height).saturating_add(1),
                stack.width,
                1,
            ),
        );
    }
}

/// Renders a one-line fallback for terminals too small for a card.
fn render_compact(
    frame: &mut Frame<'_>,
    area: Rect,
    title: &str,
    body: &[Line<'static>],
    buttons: &[Button],
) {
    let mut text = title.to_owned();
    if let Some(first) = body.first() {
        text.push_str(" · ");
        text.push_str(
            &first
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>(),
        );
    }
    let mut line = Line::from(Span::styled(
        text,
        Style::new().fg(TEXT).add_modifier(Modifier::BOLD),
    ));
    if let Some(focused) = buttons.iter().find(|button| button.focused) {
        line.spans.push(Span::raw(" "));
        line.spans.push(Span::styled(
            format!("[{}]", focused.label),
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        ));
    }
    frame.render_widget(
        Paragraph::new(line)
            .alignment(Alignment::Center)
            .style(Style::new().bg(SURFACE)),
        area,
    );
}

/// Renders one background-filled line inside a dialog.
fn render_panel_line(frame: &mut Frame<'_>, area: Rect, line: Line<'_>) {
    frame.render_widget(Paragraph::new(line).style(Style::new().bg(SURFACE)), area);
}
