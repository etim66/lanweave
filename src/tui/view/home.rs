use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Wrap};

use crate::app::model::AppModel;

use super::layout::{centered_rect, inset_surface, surface_width};
use super::presenter::screen_content;
use super::render_focus_rail;
use super::theme::{MUTED, SURFACE, TEXT, WARNING};

/// Renders the home screen: brand, state surface, hints, and tip.
///
/// Falls back to a single centered line on very small terminals.
pub(super) fn render(frame: &mut Frame<'_>, area: Rect, model: &AppModel) {
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
    let desired_height = if compact { 7 } else { 13 };
    let stack = centered_rect(area, surface_width(area), desired_height);

    render_brand(frame, Rect::new(stack.x, stack.y, stack.width, 1));

    let panel_offset = if compact { 2 } else { 3 };
    let panel_height = if compact { 3 } else { 5 };
    let panel_y = stack.y.saturating_add(panel_offset);
    let stack_bottom = stack.y.saturating_add(stack.height);
    let available_height = stack_bottom.saturating_sub(panel_y);
    if available_height > 0 {
        render_state_surface(
            frame,
            Rect::new(
                stack.x,
                panel_y,
                stack.width,
                panel_height.min(available_height),
            ),
            model,
        );
    }

    let hints_y = panel_y.saturating_add(panel_height).saturating_add(1);
    if hints_y < stack_bottom {
        render_hints(frame, Rect::new(stack.x, hints_y, stack.width, 1));
    }

    let tip_y = hints_y.saturating_add(2);
    if !compact && tip_y < stack_bottom {
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

/// Renders the centered surface with the screen title, message, and focus rail.
fn render_state_surface(frame: &mut Frame<'_>, area: Rect, model: &AppModel) {
    frame.render_widget(Block::new().style(Style::new().bg(SURFACE)), area);
    render_focus_rail(frame, area);

    if area.width < 2 || area.height == 0 {
        return;
    }

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

/// Renders the one-line keyboard hints below the state surface.
fn render_hints(frame: &mut Frame<'_>, area: Rect) {
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("/", Style::new().fg(TEXT)),
            Span::styled(" commands   ", Style::new().fg(MUTED)),
            Span::styled("q", Style::new().fg(TEXT)),
            Span::styled(" quit   ", Style::new().fg(MUTED)),
            Span::styled("ctrl+c", Style::new().fg(TEXT)),
            Span::styled(" quit", Style::new().fg(MUTED)),
        ]))
        .alignment(Alignment::Left),
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
