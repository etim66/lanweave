use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::command_palette::{CommandAvailability, registry};
use crate::app::model::AppModel;

use super::dialog;
use super::layout::surface_width;
use super::theme::{ACCENT, MUTED, TEXT};

/// Renders the help dialog with keyboard hints and command availability.
///
/// Falls back to a bare "Help" line on very small terminals.
pub(super) fn render(frame: &mut Frame<'_>, area: Rect, model: &AppModel) {
    if area.width < 12 || area.height < 5 {
        frame.render_widget(
            Paragraph::new("Help")
                .alignment(Alignment::Center)
                .style(Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)),
            area,
        );
        return;
    }

    let narrow = surface_width(area) < 56;
    let mut lines = vec![
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

    let hints = Line::from(vec![
        Span::styled("/", Style::new().fg(TEXT)),
        Span::styled(" commands   ", Style::new().fg(MUTED)),
        Span::styled("q", Style::new().fg(TEXT)),
        Span::styled(" quit", Style::new().fg(MUTED)),
    ]);

    let spec = dialog::DialogSpec::new("Keyboard & commands", ACCENT)
        .header_hint("esc")
        .footer_hint(hints);
    dialog::render_dialog(frame, area, spec, lines);
}
