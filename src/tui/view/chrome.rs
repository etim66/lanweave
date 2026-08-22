use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::widgets::Paragraph;

use crate::app::model::AppModel;

use super::presenter::status_text;
use super::theme::MUTED;

/// Renders the footer with the status label and version on the right.
///
/// The version is dropped when the terminal is too narrow.
pub(super) fn render_footer(frame: &mut Frame<'_>, area: Rect, model: &AppModel) {
    let horizontal_padding = u16::from(area.width >= 8) * 2;
    let inner = Rect::new(
        area.x.saturating_add(horizontal_padding),
        area.y,
        area.width
            .saturating_sub(horizontal_padding.saturating_mul(2)),
        1,
    );
    let version = format!("v{}", env!("CARGO_PKG_VERSION"));

    if inner.width
        >= u16::try_from(version.len())
            .unwrap_or(u16::MAX)
            .saturating_add(20)
    {
        let [status, version_area] = Layout::horizontal([
            Constraint::Min(1),
            Constraint::Length(u16::try_from(version.len()).unwrap_or(u16::MAX)),
        ])
        .areas(inner);
        frame.render_widget(
            Paragraph::new(status_text(model.state())).style(Style::new().fg(MUTED)),
            status,
        );
        frame.render_widget(
            Paragraph::new(version)
                .alignment(Alignment::Right)
                .style(Style::new().fg(MUTED)),
            version_area,
        );
    } else {
        frame.render_widget(
            Paragraph::new(status_text(model.state())).style(Style::new().fg(MUTED)),
            inner,
        );
    }
}
