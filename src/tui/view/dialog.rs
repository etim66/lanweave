use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap};

use super::layout::{centered_rect, surface_width};
use super::theme::{BACKGROUND, BORDER, HIGHLIGHT, MUTED, SURFACE, SURFACE_FOOTER, TEXT};

/// Blank rows kept between panel content and its action row, and between the
/// action row and the panel edge.
pub(super) const BUTTON_MARGIN: u16 = 1;

/// Horizontal padding between the dialog border and its content.
const HORIZONTAL_PADDING: u16 = 2;

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

/// The shared chrome and footer content for a modal dialog.
#[derive(Debug, Clone)]
pub(super) struct DialogSpec<'a> {
    title: &'a str,
    title_color: Color,
    header_hint: Option<&'a str>,
    footer_hint: Option<Line<'a>>,
    buttons: &'a [Button],
}

impl<'a> DialogSpec<'a> {
    /// Builds a dialog with no footer content.
    pub(super) const fn new(title: &'a str, title_color: Color) -> Self {
        Self {
            title,
            title_color,
            header_hint: None,
            footer_hint: None,
            buttons: &[],
        }
    }

    /// Shows a dismiss hint on the right of the title row.
    pub(super) const fn header_hint(mut self, hint: &'a str) -> Self {
        self.header_hint = Some(hint);
        self
    }

    /// Shows a shortcut hint on the left of the footer.
    pub(super) fn footer_hint(mut self, hint: Line<'a>) -> Self {
        self.footer_hint = Some(hint);
        self
    }

    /// Shows right-aligned action buttons in the footer.
    pub(super) const fn buttons(mut self, buttons: &'a [Button]) -> Self {
        self.buttons = buttons;
        self
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

/// Builds the button row: the focused action is accent-filled while the
/// others stay plain and muted.
pub(super) fn buttons_line(buttons: &[Button]) -> Line<'static> {
    let mut spans = Vec::with_capacity(buttons.len() * 2);
    for (index, button) in buttons.iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw("  "));
        }
        let style = if button.focused {
            Style::new()
                .bg(HIGHLIGHT)
                .fg(BACKGROUND)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(MUTED)
        };
        spans.push(Span::styled(format!(" {} ", button.label), style));
    }
    Line::from(spans)
}

/// Renders a modal dialog with a simple line body.
///
/// The card has a rounded border, a title row with an optional dismiss hint,
/// padded content, and a footer bar with the action buttons aligned right.
pub(super) fn render_dialog(
    frame: &mut Frame<'_>,
    area: Rect,
    spec: DialogSpec<'_>,
    body: Vec<Line<'static>>,
) -> Option<Rect> {
    if area.width < 12 || area.height < 5 {
        render_compact(frame, area, spec.title, &body, spec.buttons);
        return None;
    }

    let body_height = measure_lines(&body, content_width(area));
    render_shell(frame, area, spec, body_height, |frame, body_area| {
        frame.render_widget(
            Paragraph::new(body)
                .wrap(Wrap { trim: false })
                .style(Style::new().bg(SURFACE)),
            body_area,
        );
    })
}

/// Renders a modal dialog whose body is drawn by the caller.
///
/// `body_height` is the number of content rows the caller wants below the
/// header; the footer and border are added around it.
pub(super) fn render_dialog_with<F>(
    frame: &mut Frame<'_>,
    area: Rect,
    spec: DialogSpec<'_>,
    body_height: u16,
    body: F,
) -> Option<Rect>
where
    F: FnOnce(&mut Frame<'_>, Rect),
{
    if area.width < 12 || area.height < 5 {
        return None;
    }

    render_shell(frame, area, spec, body_height, body)
}

/// Draws the shared dialog chrome and delegates the body to `body`.
fn render_shell<F>(
    frame: &mut Frame<'_>,
    area: Rect,
    spec: DialogSpec<'_>,
    body_height: u16,
    body: F,
) -> Option<Rect>
where
    F: FnOnce(&mut Frame<'_>, Rect),
{
    let DialogSpec {
        title,
        title_color,
        header_hint,
        footer_hint,
        buttons,
    } = spec;
    let has_footer = !buttons.is_empty() || footer_hint.is_some();
    let chrome = if has_footer { 7 } else { 5 };
    let desired = body_height.max(1).saturating_add(chrome);
    let card = centered_rect(area, surface_width(area), desired.min(area.height));

    frame.render_widget(Clear, card);
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(BORDER))
        .style(Style::new().bg(SURFACE));
    let inner = block.inner(card);
    frame.render_widget(block, card);

    if inner.width < HORIZONTAL_PADDING * 2 + 1 || inner.height < 3 {
        return None;
    }

    let content = Rect::new(
        inner.x.saturating_add(HORIZONTAL_PADDING),
        inner.y,
        inner.width.saturating_sub(HORIZONTAL_PADDING * 2),
        inner.height,
    );
    render_header(
        frame,
        Rect::new(content.x, content.y, content.width, 1),
        title,
        title_color,
        header_hint,
    );

    let footer_y = inner.y + inner.height - 1;
    // Room for the header, a blank row, a footer divider, and the footer.
    let full_chrome = inner.height >= 5;
    if full_chrome {
        let divider_y = footer_y.saturating_sub(1);
        let body_top = content.y.saturating_add(2);
        let body_bottom = if has_footer {
            divider_y.saturating_sub(1)
        } else {
            content.y + content.height.saturating_sub(1)
        };
        let body_area = Rect::new(
            content.x,
            body_top,
            content.width,
            body_bottom.saturating_sub(body_top),
        );
        if body_area.height > 0 {
            body(frame, body_area);
        }

        if has_footer {
            let bar = Rect::new(inner.x, divider_y, inner.width, 2);
            frame.render_widget(Block::new().style(Style::new().bg(SURFACE_FOOTER)), bar);
            let divider = "─".repeat(usize::from(inner.width));
            frame.render_widget(
                Paragraph::new(Line::styled(divider, Style::new().fg(BORDER)))
                    .style(Style::new().bg(SURFACE_FOOTER)),
                Rect::new(inner.x, divider_y, inner.width, 1),
            );
            render_footer(
                frame,
                Rect::new(content.x, footer_y, content.width, 1),
                footer_hint,
                buttons,
            );
        }
        return Some(body_area);
    }

    // Short terminals keep the header and footer but lose the blank rows and
    // divider so the body still has a couple of usable rows.
    let body_top = content.y.saturating_add(1);
    let body_bottom = if has_footer {
        footer_y
    } else {
        inner.y + inner.height
    };
    let body_area = Rect::new(
        content.x,
        body_top,
        content.width,
        body_bottom.saturating_sub(body_top),
    );
    if body_area.height > 0 {
        body(frame, body_area);
    }
    if has_footer {
        frame.render_widget(
            Block::new().style(Style::new().bg(SURFACE_FOOTER)),
            Rect::new(inner.x, footer_y, inner.width, 1),
        );
        render_footer(
            frame,
            Rect::new(content.x, footer_y, content.width, 1),
            footer_hint,
            buttons,
        );
    }

    Some(body_area)
}

/// Renders the title on the left and the dismiss hint on the right.
fn render_header(
    frame: &mut Frame<'_>,
    area: Rect,
    title: &str,
    title_color: Color,
    header_hint: Option<&str>,
) {
    frame.render_widget(
        Paragraph::new(Line::styled(
            title,
            Style::new().fg(title_color).add_modifier(Modifier::BOLD),
        ))
        .style(Style::new().bg(SURFACE)),
        area,
    );

    if let Some(hint) = header_hint {
        frame.render_widget(
            Paragraph::new(Line::styled(hint, Style::new().fg(MUTED)))
                .alignment(Alignment::Right)
                .style(Style::new().bg(SURFACE)),
            area,
        );
    }
}

/// Renders the left-side shortcut hint and right-aligned action buttons.
fn render_footer(frame: &mut Frame<'_>, area: Rect, hint: Option<Line<'_>>, buttons: &[Button]) {
    let style = Style::new().bg(SURFACE_FOOTER);
    if buttons.is_empty() {
        if let Some(hint) = hint {
            frame.render_widget(Paragraph::new(hint).style(style), area);
        }
        return;
    }

    let buttons_width = buttons_width(buttons);
    if let Some(hint) = hint
        && area.width > buttons_width.saturating_add(2)
    {
        let hint_area = Rect::new(
            area.x,
            area.y,
            area.width.saturating_sub(buttons_width.saturating_add(2)),
            1,
        );
        frame.render_widget(Paragraph::new(hint).style(style), hint_area);
    }
    frame.render_widget(
        Paragraph::new(buttons_line(buttons))
            .alignment(Alignment::Right)
            .style(style),
        area,
    );
}

/// Returns the rendered width of a button row.
fn buttons_width(buttons: &[Button]) -> u16 {
    if buttons.is_empty() {
        return 0;
    }
    let labels: usize = buttons
        .iter()
        .map(|button| button.label.chars().count())
        .sum();
    let padding = buttons.len() * 2;
    let gaps = buttons.len().saturating_sub(1) * 2;
    u16::try_from(labels.saturating_add(padding).saturating_add(gaps)).unwrap_or(u16::MAX)
}

/// Returns the usable content width for a dialog in `area`.
fn content_width(area: Rect) -> u16 {
    surface_width(area)
        .saturating_sub(2 + HORIZONTAL_PADDING * 2)
        .max(1)
}

/// Estimates the wrapped row count for a simple dialog body.
fn measure_lines(lines: &[Line<'static>], width: u16) -> u16 {
    let width = usize::from(width.max(1));
    let mut rows = 0usize;

    for line in lines {
        let text: String = line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        let mut current = 0usize;
        let mut line_rows = 1usize;
        for word in text.split_inclusive(' ') {
            let word_len = word.chars().count();
            if current > 0 && current.saturating_add(word_len) > width {
                line_rows += 1;
                current = 0;
            }
            if word_len > width {
                let extra = word_len.saturating_sub(width);
                line_rows += extra.div_ceil(width);
                current = word_len % width.max(1);
            } else {
                current = current.saturating_add(word_len);
            }
        }
        rows = rows.saturating_add(line_rows);
    }

    u16::try_from(rows.max(1)).unwrap_or(u16::MAX)
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
            Style::new().bg(HIGHLIGHT).fg(BACKGROUND),
        ));
    }
    frame.render_widget(
        Paragraph::new(line)
            .alignment(Alignment::Center)
            .style(Style::new().bg(SURFACE)),
        area,
    );
}
