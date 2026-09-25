use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use crate::app::command_palette::{CommandAvailability, CommandSpec, visible_commands};
use crate::app::interaction::CommandPalette;
use crate::app::model::AppModel;

use super::layout::{centered_rect, inset_surface, scroll_window, surface_width};
use super::presenter::status_text;
use super::render_focus_rail;
use super::theme::{ACCENT, BACKGROUND, HIGHLIGHT, MUTED, SURFACE, TEXT};

/// Renders the command palette card with filtered rows, query, and hints.
///
/// Falls back to the raw query line on very small terminals.
pub(super) fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    model: &AppModel,
    palette: &CommandPalette,
) {
    if area.width < 8 || area.height < 3 {
        frame.render_widget(
            Paragraph::new(format!("/{}", palette.query))
                .alignment(Alignment::Center)
                .style(Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)),
            area,
        );
        return;
    }

    let commands = visible_commands(model.capabilities(), &palette.query);
    let result_rows = u16::try_from(commands.len().max(1)).unwrap_or(u16::MAX);
    let desired_card_height = result_rows.saturating_add(3);
    let desired_stack_height = desired_card_height.saturating_add(2);
    let stack = centered_rect(area, surface_width(area), desired_stack_height);
    let show_hints = stack.height >= 5;
    let card_height = stack.height.saturating_sub(if show_hints { 2 } else { 0 });
    let card = Rect::new(stack.x, stack.y, stack.width, card_height);
    frame.render_widget(Block::new().style(Style::new().bg(SURFACE)), card);

    let query_height = 3.min(card.height);
    let command_capacity = card.height.saturating_sub(query_height);
    render_rows(frame, card, command_capacity, model, palette, &commands);

    let query_area = Rect::new(
        card.x,
        card.y.saturating_add(command_capacity),
        card.width,
        query_height,
    );
    render_query(frame, query_area, model, palette);

    if show_hints {
        let hints = Rect::new(
            stack.x,
            stack.y.saturating_add(card.height).saturating_add(1),
            stack.width,
            1,
        );
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("up/down", Style::new().fg(TEXT)),
                Span::styled(" select   ", Style::new().fg(MUTED)),
                Span::styled("enter", Style::new().fg(TEXT)),
                Span::styled(" run   ", Style::new().fg(MUTED)),
                Span::styled("esc", Style::new().fg(TEXT)),
                Span::styled(" close", Style::new().fg(MUTED)),
            ])),
            hints,
        );
    }
}

/// Renders the command rows, scrolled around the selected command.
fn render_rows(
    frame: &mut Frame<'_>,
    card: Rect,
    capacity: u16,
    model: &AppModel,
    palette: &CommandPalette,
    commands: &[&CommandSpec],
) {
    if capacity == 0 {
        return;
    }

    if commands.is_empty() {
        let row = Rect::new(card.x, card.y, card.width, 1);
        frame.render_widget(
            Paragraph::new("  No matching commands").style(Style::new().bg(SURFACE).fg(MUTED)),
            row,
        );
        return;
    }

    let capacity = usize::from(capacity);
    let selected_index = palette
        .selected
        .and_then(|selected| commands.iter().position(|command| command.id == selected))
        .unwrap_or(0);
    let (start, _) = scroll_window(commands.len(), selected_index, capacity);

    for (row_index, command) in commands.iter().skip(start).take(capacity).enumerate() {
        let availability = (command.availability)(model.capabilities());
        let selected = palette.selected == Some(command.id);
        let detail = match availability {
            CommandAvailability::Disabled(reason) => reason,
            _ => command.description,
        };
        let row_style = match (selected, availability) {
            (true, CommandAvailability::Enabled) => Style::new().bg(HIGHLIGHT).fg(BACKGROUND),
            (true, _) => Style::new().bg(MUTED).fg(BACKGROUND),
            (false, CommandAvailability::Disabled(_)) => Style::new().bg(SURFACE).fg(MUTED),
            _ => Style::new().bg(SURFACE).fg(TEXT),
        };
        let row = Rect::new(
            card.x,
            card.y
                .saturating_add(u16::try_from(row_index).unwrap_or(u16::MAX)),
            card.width,
            1,
        );
        let line = if row.width >= 32 {
            Line::from(vec![
                Span::styled(
                    format!("  {:<13}", command.name),
                    Style::new().add_modifier(Modifier::BOLD),
                ),
                Span::raw(detail),
            ])
        } else {
            Line::from(Span::styled(
                format!("  {}", command.name),
                Style::new().add_modifier(Modifier::BOLD),
            ))
        };
        frame.render_widget(Paragraph::new(line).style(row_style), row);
    }
}

/// Renders the query line and status row at the bottom of the palette.
fn render_query(frame: &mut Frame<'_>, area: Rect, model: &AppModel, palette: &CommandPalette) {
    if area.height == 0 {
        return;
    }

    render_focus_rail(frame, area);
    let inner = inset_surface(area, 0);
    frame.render_widget(
        Paragraph::new(format!("/{}", palette.query)).style(
            Style::new()
                .bg(SURFACE)
                .fg(TEXT)
                .add_modifier(Modifier::BOLD),
        ),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );

    if area.height >= 3 {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("Lanweave", Style::new().fg(ACCENT)),
                Span::styled("  /  ", Style::new().fg(MUTED)),
                Span::styled(status_text(model.state()), Style::new().fg(MUTED)),
            ]))
            .style(Style::new().bg(SURFACE)),
            Rect::new(inner.x, inner.y + 2, inner.width, 1),
        );
    }
}
