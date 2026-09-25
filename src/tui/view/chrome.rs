use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::model::AppModel;
use crate::discovery::escape_display;

use super::presenter::status_text;
use super::theme::{ACCENT, MUTED};

/// Renders the footer with the working directory, status, and version.
///
/// The working directory is shortened with `~` and dropped before the status
/// when the terminal is narrow.
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
    let version_width = u16::try_from(version.len()).unwrap_or(u16::MAX);
    // " · " is three display columns between the status text and the version.
    let status_width = u16::try_from(status_text(model.state()).len() + version.len())
        .unwrap_or(u16::MAX)
        .saturating_add(3);

    // A roomy footer shows the directory where Lanweave was opened on the
    // left and the state with the version on the right.
    if inner.width >= status_width.saturating_add(20) {
        let directory = shorten_home(&model.working_directory().display().to_string());
        let [directory_area, status_area] =
            Layout::horizontal([Constraint::Min(1), Constraint::Length(status_width)]).areas(inner);
        let directory_line = match directory.strip_prefix('~') {
            Some(rest) => Line::from(vec![
                Span::styled("~", Style::new().fg(ACCENT)),
                Span::styled(rest.to_owned(), Style::new().fg(MUTED)),
            ]),
            None => Line::styled(directory, Style::new().fg(MUTED)),
        };
        frame.render_widget(Paragraph::new(directory_line), directory_area);
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(status_text(model.state()), Style::new().fg(MUTED)),
                Span::styled(" · ", Style::new().fg(MUTED)),
                Span::styled(version, Style::new().fg(MUTED)),
            ]))
            .alignment(Alignment::Right),
            status_area,
        );
        return;
    }

    if inner.width >= version_width.saturating_add(20) {
        let [status_area, version_area] =
            Layout::horizontal([Constraint::Min(1), Constraint::Length(version_width)])
                .areas(inner);
        frame.render_widget(
            Paragraph::new(status_text(model.state())).style(Style::new().fg(MUTED)),
            status_area,
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

/// Replaces a matching home-directory prefix with `~`.
///
/// The displayed path is escaped because local paths may contain control
/// characters in unusual setups.
pub(super) fn shorten_home(path: &str) -> String {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_default();
    let shortened = if !home.is_empty() && path.starts_with(&home) {
        format!("~{}", &path[home.len()..])
    } else {
        path.to_owned()
    };
    escape_display(&shortened)
}
