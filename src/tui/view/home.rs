use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Wrap};

use crate::app::interaction::UiState;
use crate::app::model::{AppModel, Screen};

use super::layout::{centered_rect, inset_surface, surface_width};
use super::presenter::screen_content;
use super::render_focus_rail;
use super::theme::{ACCENT, BACKGROUND, HIGHLIGHT, MUTED, SURFACE, TEXT, WARNING};

/// Renders the home screen: brand, state surface, hints, and tip.
///
/// The browsing screen grows a device list panel; every other screen keeps
/// the centered message surface. Falls back to a single centered line on very
/// small terminals.
pub(super) fn render(frame: &mut Frame<'_>, area: Rect, model: &AppModel, ui: &UiState) {
    if area.width < 12 || area.height < 5 {
        frame.render_widget(
            Paragraph::new(screen_content(model).0)
                .alignment(Alignment::Center)
                .style(Style::new().fg(TEXT).add_modifier(Modifier::BOLD)),
            area,
        );
        return;
    }

    let compact = area.width < 60 || area.height < 16;
    let browsing = model.screen() == Screen::Browsing;
    let desired_height = if browsing {
        18
    } else if compact {
        7
    } else {
        13
    };
    let stack = centered_rect(area, surface_width(area), desired_height);

    render_brand(frame, Rect::new(stack.x, stack.y, stack.width, 1));

    let panel_offset = if compact { 2 } else { 3 };
    let panel_y = stack.y.saturating_add(panel_offset);
    let stack_bottom = stack.y.saturating_add(stack.height);
    let available_height = stack_bottom.saturating_sub(panel_y);

    let panel_height = if browsing {
        available_height.saturating_sub(if compact { 1 } else { 2 })
    } else {
        (if compact { 3 } else { 5 }).min(available_height)
    };
    if panel_height > 0 {
        render_state_surface(
            frame,
            Rect::new(stack.x, panel_y, stack.width, panel_height),
            model,
            ui,
        );
    }

    let hints_y = panel_y.saturating_add(panel_height).saturating_add(1);
    if hints_y < stack_bottom {
        render_hints(frame, Rect::new(stack.x, hints_y, stack.width, 1), browsing);
    }

    let tip_y = hints_y.saturating_add(2);
    if !compact && !browsing && tip_y < stack_bottom {
        render_tip(frame, Rect::new(stack.x, tip_y, stack.width, 1));
    }
}

/// Renders the two-tone "lanweave" brand line.
fn render_brand(frame: &mut Frame<'_>, area: Rect) {
    let brand = Line::from(vec![
        Span::styled("lan", Style::new().fg(MUTED).add_modifier(Modifier::BOLD)),
        Span::styled("weave", Style::new().fg(TEXT).add_modifier(Modifier::BOLD)),
    ]);
    frame.render_widget(Paragraph::new(brand).alignment(Alignment::Center), area);
}

/// Renders the centered surface: device list while browsing, message otherwise.
fn render_state_surface(frame: &mut Frame<'_>, area: Rect, model: &AppModel, ui: &UiState) {
    frame.render_widget(Block::new().style(Style::new().bg(SURFACE)), area);
    render_focus_rail(frame, area);

    if area.width < 2 || area.height == 0 {
        return;
    }

    if model.screen() == Screen::Browsing {
        render_device_panel(frame, area, model, ui);
    } else {
        render_message_panel(frame, area, model);
    }
}

/// Renders the title and message for a non-browsing screen.
fn render_message_panel(frame: &mut Frame<'_>, area: Rect, model: &AppModel) {
    let (title, message, color) = screen_content(model);
    let top_padding = u16::from(area.height >= 4);
    let inner = inset_surface(area, top_padding);
    let lines = vec![
        Line::styled(title, Style::new().fg(color).add_modifier(Modifier::BOLD)),
        Line::styled(message, Style::new().fg(MUTED)),
    ];
    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::new().bg(SURFACE))
            .wrap(Wrap { trim: true }),
        inner,
    );
}

/// Renders the live device list: title, rows, and the untrusted note.
///
/// Rows are scrolled around the selected device so the highlight never leaves
/// the panel. The empty state explains that discovery is still searching.
fn render_device_panel(frame: &mut Frame<'_>, area: Rect, model: &AppModel, ui: &UiState) {
    let inner = inset_surface(area, u16::from(area.height >= 4));
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let candidates = model.sorted_candidates();
    let (title, title_color) = if candidates.is_empty() {
        ("No devices found", WARNING)
    } else {
        ("Devices", ACCENT)
    };
    render_panel_line(
        frame,
        Rect::new(inner.x, inner.y, inner.width, 1),
        Line::styled(
            title,
            Style::new().fg(title_color).add_modifier(Modifier::BOLD),
        ),
    );

    if candidates.is_empty() {
        render_panel_line(
            frame,
            Rect::new(
                inner.x,
                inner.y + 1,
                inner.width,
                inner.height.saturating_sub(1),
            ),
            Line::styled("Searching the local network...", Style::new().fg(MUTED)),
        );
        return;
    }

    let note_height = u16::from(inner.height >= 3);
    let row_capacity = inner.height.saturating_sub(2).saturating_sub(note_height);
    if row_capacity == 0 {
        return;
    }

    let selected = ui.device_selection();
    let selected_index = selected.and_then(|selected| {
        candidates
            .iter()
            .position(|candidate| candidate.id() == selected)
    });
    let start = selected_index
        .unwrap_or(0)
        .saturating_add(1)
        .saturating_sub(usize::from(row_capacity));

    for (offset, candidate) in candidates
        .iter()
        .skip(start)
        .take(usize::from(row_capacity))
        .enumerate()
    {
        render_device_row(
            frame,
            Rect::new(
                inner.x,
                inner.y + 1 + u16::try_from(offset).unwrap_or(u16::MAX),
                inner.width,
                1,
            ),
            candidate,
            selected == Some(candidate.id()),
        );
    }

    if note_height > 0 {
        render_panel_line(
            frame,
            Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1),
            Line::from(vec![
                Span::styled("?", Style::new().fg(WARNING).add_modifier(Modifier::BOLD)),
                Span::styled(
                    " untrusted until pairing confirms the device",
                    Style::new().fg(MUTED),
                ),
            ]),
        );
    }
}

/// Renders one device row with the selection highlight.
fn render_device_row(
    frame: &mut Frame<'_>,
    area: Rect,
    candidate: &crate::discovery::Candidate,
    is_selected: bool,
) {
    let row_style = if is_selected {
        Style::new().bg(HIGHLIGHT).fg(BACKGROUND)
    } else {
        Style::new().bg(SURFACE).fg(TEXT)
    };
    let name_style = row_style.add_modifier(Modifier::BOLD);

    let line = if area.width >= 40 {
        Line::from(vec![
            Span::styled(format!("  {}", candidate.display_name()), name_style),
            Span::styled(
                format!("  {}:{}", candidate.host(), candidate.port()),
                row_style,
            ),
        ])
    } else {
        Line::from(Span::styled(
            format!("  {}", candidate.display_name()),
            name_style,
        ))
    };
    frame.render_widget(Paragraph::new(line).style(row_style), area);
}

/// Renders one background-filled line inside the device panel.
fn render_panel_line(frame: &mut Frame<'_>, area: Rect, line: Line<'_>) {
    frame.render_widget(Paragraph::new(line).style(Style::new().bg(SURFACE)), area);
}

/// Renders the one-line keyboard hints below the state surface.
fn render_hints(frame: &mut Frame<'_>, area: Rect, browsing: bool) {
    let spans = if browsing {
        vec![
            Span::styled("up/down", Style::new().fg(TEXT)),
            Span::styled(" select   ", Style::new().fg(MUTED)),
            Span::styled("enter", Style::new().fg(TEXT)),
            Span::styled(" connect   ", Style::new().fg(MUTED)),
            Span::styled("/", Style::new().fg(TEXT)),
            Span::styled(" commands   ", Style::new().fg(MUTED)),
            Span::styled("q", Style::new().fg(TEXT)),
            Span::styled(" quit", Style::new().fg(MUTED)),
        ]
    } else {
        vec![
            Span::styled("/", Style::new().fg(TEXT)),
            Span::styled(" commands   ", Style::new().fg(MUTED)),
            Span::styled("q", Style::new().fg(TEXT)),
            Span::styled(" quit   ", Style::new().fg(MUTED)),
            Span::styled("ctrl+c", Style::new().fg(TEXT)),
            Span::styled(" quit", Style::new().fg(MUTED)),
        ]
    };
    frame.render_widget(
        Paragraph::new(Line::from(spans)).alignment(Alignment::Left),
        area,
    );
}

/// Renders the "Tip" line shown on roomy terminals.
fn render_tip(frame: &mut Frame<'_>, area: Rect) {
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "Tip ",
                Style::new().fg(WARNING).add_modifier(Modifier::BOLD),
            ),
            Span::styled("Open commands with ", Style::new().fg(MUTED)),
            Span::styled("/", Style::new().fg(TEXT)),
        ]))
        .alignment(Alignment::Center),
        area,
    );
}
