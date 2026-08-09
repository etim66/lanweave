use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Wrap};

use crate::app::command_palette::{CommandAvailability, registry};
use crate::app::model::AppModel;

use super::layout::{centered_rect, inset_surface, surface_width};
use super::render_focus_rail;
use super::theme::{ACCENT, MUTED, SURFACE, TEXT};

pub(super) fn render(frame: &mut Frame<'_>, area: Rect, model: &AppModel) {
    if area.width < 8 || area.height < 3 {
        frame.render_widget(
            Paragraph::new("Help")
                .alignment(Alignment::Center)
                .style(Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)),
            area,
        );
        return;
    }

    let desired_height = u16::try_from(registry().len())
        .unwrap_or(u16::MAX)
        .saturating_add(6);
    let surface = centered_rect(area, surface_width(area), desired_height);
    frame.render_widget(Block::new().style(Style::new().bg(SURFACE)), surface);
    render_focus_rail(frame, surface);

    let narrow = surface.width < 50;
    let mut lines = vec![
        Line::styled(
            "Keyboard & commands",
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Line::styled(
            "Type / to filter commands, then press Enter to run one.",
            Style::new().fg(MUTED),
        ),
        Line::default(),
    ];
    for command in registry() {
        let availability = match (command.availability)(model.capabilities()) {
            CommandAvailability::Enabled => "available",
            CommandAvailability::Disabled(reason) => reason,
            CommandAvailability::Hidden => "unavailable on this screen",
        };
        if narrow {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{:<13}", command.name),
                    Style::new().fg(TEXT).add_modifier(Modifier::BOLD),
                ),
                Span::styled(availability, Style::new().fg(MUTED)),
            ]));
        } else {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{:<13}", command.name),
                    Style::new().fg(TEXT).add_modifier(Modifier::BOLD),
                ),
                Span::styled(command.description, Style::new().fg(MUTED)),
                Span::styled(format!("  {availability}"), Style::new().fg(MUTED)),
            ]));
        }
    }
    lines.push(Line::default());
    lines.push(Line::from(vec![
        Span::styled("esc", Style::new().fg(TEXT)),
        Span::styled(" close   ", Style::new().fg(MUTED)),
        Span::styled("/", Style::new().fg(TEXT)),
        Span::styled(" commands   ", Style::new().fg(MUTED)),
        Span::styled("q", Style::new().fg(TEXT)),
        Span::styled(" quit", Style::new().fg(MUTED)),
    ]));

    let inner = inset_surface(surface, 1);
    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::new().bg(SURFACE))
            .wrap(Wrap { trim: true }),
        inner,
    );
}
