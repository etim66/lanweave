use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::command_palette::{CommandAvailability, CommandSpec, visible_commands};
use crate::app::interaction::CommandPalette;
use crate::app::model::AppModel;

use super::dialog;
use super::layout::scroll_window;
use super::theme::{ACCENT, BACKGROUND, HIGHLIGHT, MUTED, SURFACE, TEXT};

/// Renders the command palette with filtered rows and the live query.
///
/// Falls back to the raw query line on very small terminals.
pub(super) fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    model: &AppModel,
    palette: &CommandPalette,
) {
    if area.width < 12 || area.height < 5 {
        frame.render_widget(
            Paragraph::new(format!("/{}", palette.query))
                .alignment(Alignment::Center)
                .style(Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)),
            area,
        );
        return;
    }

    let commands = visible_commands(model.capabilities(), &palette.query);
    let visible_rows = u16::try_from(commands.len().clamp(1, 10)).unwrap_or(1);
    let hints = Line::from(vec![
        Span::styled("up/down", Style::new().fg(TEXT)),
        Span::styled(" select   ", Style::new().fg(MUTED)),
        Span::styled("enter", Style::new().fg(TEXT)),
        Span::styled(" run", Style::new().fg(MUTED)),
    ]);

    let spec = dialog::DialogSpec::new("Commands", ACCENT)
        .header_hint("esc")
        .footer_hint(hints);
    dialog::render_dialog_with(
        frame,
        area,
        spec,
        visible_rows.saturating_add(2),
        |frame, body| render_body(frame, body, model, palette, &commands),
    );
}

/// Renders the command rows and the query line inside the dialog body.
fn render_body(
    frame: &mut Frame<'_>,
    body: Rect,
    model: &AppModel,
    palette: &CommandPalette,
    commands: &[&CommandSpec],
) {
    if body.width == 0 || body.height == 0 {
        return;
    }

    let query_y = body.y + body.height - 1;
    let capacity = usize::from(body.height.saturating_sub(2));
    render_rows(
        frame,
        Rect::new(body.x, body.y, body.width, body.height.saturating_sub(2)),
        capacity,
        model,
        palette,
        commands,
    );

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("/", Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled(
                palette.query.clone(),
                Style::new().fg(TEXT).add_modifier(Modifier::BOLD),
            ),
        ]))
        .style(Style::new().bg(SURFACE)),
        Rect::new(body.x, query_y, body.width, 1),
    );
}

/// Renders the command rows, scrolled around the selected command.
fn render_rows(
    frame: &mut Frame<'_>,
    area: Rect,
    capacity: usize,
    model: &AppModel,
    palette: &CommandPalette,
    commands: &[&CommandSpec],
) {
    if area.width == 0 || area.height == 0 || capacity == 0 {
        return;
    }

    if commands.is_empty() {
        frame.render_widget(
            Paragraph::new("  No matching commands").style(Style::new().bg(SURFACE).fg(MUTED)),
            Rect::new(area.x, area.y, area.width, 1),
        );
        return;
    }

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
            area.x,
            area.y
                .saturating_add(u16::try_from(row_index).unwrap_or(u16::MAX)),
            area.width,
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
