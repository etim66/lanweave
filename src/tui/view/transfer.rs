use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::model::{AppModel, AppState, TransferDirection, TransferProgress};
use crate::discovery::escape_display;

use super::chrome::shorten_home;
use super::dialog::{self, Button};
use super::layout::inset_surface;
use super::presenter::format_size;
use super::theme::{ACCENT, MUTED, PROGRESS_TRACK, SUCCESS, SURFACE, TEXT, WARNING};

/// Renders the active-transfer panel: overall bar, current file, and list.
///
/// The surrounding surface and focus rail are drawn by the caller.
pub(super) fn render_panel(frame: &mut Frame<'_>, area: Rect, model: &AppModel) {
    let inner = inset_surface(area, u16::from(area.height >= 4));
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let sending = model.state() == AppState::TransferringOutbound;
    let (verb, preposition) = if sending {
        ("Sending", "to")
    } else {
        ("Receiving", "from")
    };
    let peer = peer_label(model);
    let title = format!("{verb} {preposition} {peer}");
    render_panel_line(
        frame,
        Rect::new(inner.x, inner.y, inner.width, 1),
        Line::styled(title, Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)),
    );

    let files = tracked_files(model);
    let mut y = inner.y + 2;
    if let Some(progress) = model.transfer_progress() {
        render_status(frame, Rect::new(inner.x, y, inner.width, 1), &progress);
        y += 1;
        if y < inner.y + inner.height && inner.width > 0 {
            frame.render_widget(
                progress_bar(overall_ratio(&progress), inner.width),
                Rect::new(inner.x, y, inner.width, 1),
            );
            y += 1;
        }
    } else {
        render_panel_line(
            frame,
            Rect::new(inner.x, y, inner.width, 1),
            Line::styled(
                format!("{verb} files over the encrypted session..."),
                Style::new().fg(MUTED),
            ),
        );
        y += 1;
    }

    // The recipient can always see where files are being saved.
    if !sending && y < inner.y + inner.height {
        let destination = shorten_home(&model.default_destination().display().to_string());
        render_panel_line(
            frame,
            Rect::new(inner.x, y, inner.width, 1),
            Line::from(vec![
                Span::styled("Saving to: ", Style::new().fg(ACCENT)),
                Span::styled(destination, Style::new().fg(MUTED)),
            ]),
        );
        y += 1;
    }

    y += 1;
    let button_rows = u16::from(inner.height >= 6);
    let capacity = (inner.y + inner.height)
        .saturating_sub(y)
        .saturating_sub(button_rows);
    render_file_rows(
        frame,
        Rect::new(inner.x, y, inner.width, capacity),
        &files,
        model.transfer_progress(),
    );
    if button_rows > 0 {
        dialog::render_buttons(
            frame,
            Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1),
            &[Button::new("Cancel transfer", true)],
        );
    }
}

/// Renders the finished-transfer summary for both participants.
pub(super) fn render_summary_panel(frame: &mut Frame<'_>, area: Rect, model: &AppModel) {
    let inner = inset_surface(area, u16::from(area.height >= 4));
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let Some(summary) = model.summary() else {
        render_panel_line(
            frame,
            Rect::new(inner.x, inner.y, inner.width, 1),
            Line::styled("Transfer complete", Style::new().fg(SUCCESS)),
        );
        return;
    };

    let (title, color) = if summary.cancelled {
        ("Transfer cancelled", WARNING)
    } else {
        ("✓ Transfer complete", SUCCESS)
    };
    render_panel_line(
        frame,
        Rect::new(inner.x, inner.y, inner.width, 1),
        Line::styled(title, Style::new().fg(color).add_modifier(Modifier::BOLD)),
    );

    let (verb, preposition) = match summary.direction {
        TransferDirection::Sent => ("Sent", "to"),
        TransferDirection::Received => ("Received", "from"),
    };
    let peer = summary.peer.as_deref().unwrap_or("the other device");
    let detail = if summary.cancelled {
        format!(
            "{verb} {} file(s) ({}) {preposition} {peer}",
            summary.files.len(),
            format_size(summary.total_size)
        )
    } else {
        format!(
            "{verb} {} file(s) ({}) {preposition} {peer} in {}",
            summary.files.len(),
            format_size(summary.total_size),
            format_duration(summary.duration.as_secs())
        )
    };
    render_panel_line(
        frame,
        Rect::new(inner.x, inner.y + 1, inner.width, 1),
        Line::styled(detail, Style::new().fg(MUTED)),
    );

    // Reserve the Done button, destination, and session-note rows at the bottom.
    let reserved = u16::from(summary.destination.is_some()) + u16::from(summary.session_closed) + 1;
    let list_start = inner.y.saturating_add(3);
    let list_end = (inner.y + inner.height)
        .saturating_sub(reserved)
        .max(list_start);
    render_panel_line(
        frame,
        Rect::new(inner.x, list_start.saturating_sub(1), inner.width, 1),
        Line::styled(
            "Files",
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
    );

    if list_end > list_start {
        for (index, entry) in summary
            .files
            .iter()
            .take(usize::from(list_end - list_start))
            .enumerate()
        {
            let mut spans = vec![
                Span::styled(" ✓ ", Style::new().fg(SUCCESS)),
                Span::styled(escape_display(&entry.name), Style::new().fg(TEXT)),
                Span::styled(
                    format!("  {}", format_size(entry.size)),
                    Style::new().fg(MUTED),
                ),
            ];
            if let Some(folder) = &entry.folder {
                spans.push(Span::styled(
                    format!(
                        "  folder · {} items · {}",
                        folder.items,
                        format_size(folder.source_size)
                    ),
                    Style::new().fg(WARNING),
                ));
            }
            render_panel_line(
                frame,
                Rect::new(
                    inner.x,
                    list_start + u16::try_from(index).unwrap_or(u16::MAX),
                    inner.width,
                    1,
                ),
                Line::from(spans),
            );
        }
    }

    let bottom = inner.y + inner.height;
    // The Done button always owns the last card row.
    dialog::render_buttons(
        frame,
        Rect::new(inner.x, bottom.saturating_sub(1), inner.width, 1),
        &[Button::new("Done", true)],
    );
    let mut footer_y = bottom.saturating_sub(1);
    if summary.session_closed {
        footer_y = footer_y.saturating_sub(1);
        render_panel_line(
            frame,
            Rect::new(inner.x, footer_y, inner.width, 1),
            Line::styled(
                "The session closed; pair again for another transfer.",
                Style::new().fg(WARNING),
            ),
        );
    }
    if let Some(destination) = summary.destination.as_ref() {
        footer_y = footer_y.saturating_sub(1);
        render_panel_line(
            frame,
            Rect::new(inner.x, footer_y, inner.width, 1),
            Line::from(vec![
                Span::styled("Saved to: ", Style::new().fg(ACCENT)),
                Span::styled(
                    shorten_home(&destination.display().to_string()),
                    Style::new().fg(TEXT),
                ),
            ]),
        );
    }
}

/// Renders the "file X of Y · NN% · speed · ETA" status line.
fn render_status(frame: &mut Frame<'_>, area: Rect, progress: &TransferProgress) {
    let percent = percentage(progress.total_transferred, progress.total_size);
    let mut text = format!(
        "file {} of {} · {percent:>3}%",
        u32::from(progress.index) + 1,
        progress.files
    );
    if let Some(speed) = transfer_speed(progress) {
        text.push_str(&format!(" · {}/s", format_size(speed)));
    }
    if let Some(eta) = time_left(progress) {
        text.push_str(&format!(" · {eta} left"));
    }
    render_panel_line(
        frame,
        area,
        Line::styled(text, Style::new().fg(TEXT).add_modifier(Modifier::BOLD)),
    );
}

/// Renders manifest rows with a per-file bar for the current entry.
fn render_file_rows(
    frame: &mut Frame<'_>,
    area: Rect,
    files: &[(String, u64)],
    progress: Option<TransferProgress>,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    if files.is_empty() {
        render_panel_line(
            frame,
            area,
            Line::styled("Waiting for the file list...", Style::new().fg(MUTED)),
        );
        return;
    }

    let current = progress.map(|progress| progress.index);
    for (index, (name, size)) in files.iter().take(usize::from(area.height)).enumerate() {
        let index = u16::try_from(index).unwrap_or(u16::MAX);
        let (marker, marker_style) = match current {
            Some(current) if index < current => ("✓", Style::new().fg(SUCCESS)),
            Some(current) if index == current => ("▶", Style::new().fg(ACCENT)),
            _ => ("·", Style::new().fg(MUTED)),
        };
        let mut spans = vec![
            Span::styled(
                format!(" {:>2} ", u32::from(index) + 1),
                Style::new().fg(MUTED),
            ),
            Span::styled(
                format!("{marker} "),
                marker_style.add_modifier(Modifier::BOLD),
            ),
            Span::styled(escape_display(name), Style::new().fg(TEXT)),
            Span::styled(format!("  {}", format_size(*size)), Style::new().fg(MUTED)),
        ];
        if let Some(progress) = progress
            && index == progress.index
        {
            let ratio = ratio(progress.transferred, progress.file_size);
            spans.push(Span::styled(
                format!("  {}", bar_text(ratio, 12)),
                Style::new().fg(ACCENT),
            ));
            spans.push(Span::styled(
                format!(" {:>3}%", (ratio * 100.0).round() as u32),
                Style::new().fg(MUTED),
            ));
        }
        frame.render_widget(
            Paragraph::new(Line::from(spans)).style(Style::new().bg(SURFACE)),
            Rect::new(area.x, area.y + index, area.width, 1),
        );
    }
}

/// Returns the manifest rows of the active transfer in display order.
fn tracked_files(model: &AppModel) -> Vec<(String, u64)> {
    if model.state() == AppState::TransferringOutbound {
        return model
            .outbound_selection()
            .map(|selection| {
                selection
                    .files()
                    .iter()
                    .map(|file| (file.name().to_owned(), file.size()))
                    .collect()
            })
            .unwrap_or_default();
    }
    model
        .transfer_proposal()
        .map(|proposal| {
            proposal
                .files()
                .iter()
                .map(|entry| (entry.name.clone(), entry.size))
                .collect()
        })
        .unwrap_or_default()
}

/// Returns the confirmed peer label, bounded to the untrusted display name.
fn peer_label(model: &AppModel) -> String {
    if let Some(peer) = model.session_peer() {
        return peer
            .display_name()
            .map(|name| format!("{name} (untrusted)"))
            .unwrap_or_else(|| peer.endpoint().to_owned());
    }
    model
        .transfer_proposal()
        .and_then(|proposal| proposal.peer())
        .map(|name| format!("{name} (untrusted)"))
        .unwrap_or_else(|| "the other device".to_owned())
}

/// Returns the overall transfer ratio in `0.0..=1.0`.
fn overall_ratio(progress: &TransferProgress) -> f64 {
    ratio(progress.total_transferred, progress.total_size)
}

/// Returns a bounded ratio for `part` of `total`.
fn ratio(part: u64, total: u64) -> f64 {
    if total == 0 {
        return 1.0;
    }
    (part as f64 / total as f64).clamp(0.0, 1.0)
}

/// Returns the whole-percent value, treating an empty transfer as complete.
fn percentage(part: u64, total: u64) -> u32 {
    (ratio(part, total) * 100.0).round() as u32
}

/// Returns the average speed once at least one second has elapsed.
fn transfer_speed(progress: &TransferProgress) -> Option<u64> {
    let seconds = progress.elapsed.as_secs_f64();
    if seconds < 1.0 || progress.total_transferred == 0 {
        return None;
    }
    Some((progress.total_transferred as f64 / seconds) as u64)
}

/// Returns the estimated time left at the average speed.
fn time_left(progress: &TransferProgress) -> Option<String> {
    let speed = transfer_speed(progress)?;
    let remaining = progress
        .total_size
        .saturating_sub(progress.total_transferred);
    if remaining == 0 {
        return None;
    }
    if speed == 0 {
        return Some("--".to_owned());
    }
    Some(format_duration(remaining / speed))
}

/// Formats a whole number of seconds compactly.
fn format_duration(seconds: u64) -> String {
    if seconds < 60 {
        return format!("{seconds}s");
    }
    format!("{}m {:02}s", seconds / 60, seconds % 60)
}

/// Builds a two-tone progress bar line of the given width.
fn progress_bar(ratio: f64, width: u16) -> Line<'static> {
    let width = width.max(1);
    let filled = (ratio * f64::from(width)).round() as u16;
    let filled = filled.min(width);
    Line::from(vec![
        Span::styled("█".repeat(usize::from(filled)), Style::new().fg(ACCENT)),
        Span::styled(
            "░".repeat(usize::from(width - filled)),
            Style::new().fg(PROGRESS_TRACK),
        ),
    ])
}

/// Builds the bar text only, for inline per-file progress.
fn bar_text(ratio: f64, width: u16) -> String {
    let width = width.max(1);
    let filled = (ratio * f64::from(width)).round() as u16;
    let filled = filled.min(width);
    format!(
        "{}{}",
        "█".repeat(usize::from(filled)),
        "░".repeat(usize::from(width - filled))
    )
}

/// Renders one background-filled line inside the transfer panel.
fn render_panel_line(frame: &mut Frame<'_>, area: Rect, line: Line<'_>) {
    frame.render_widget(Paragraph::new(line).style(Style::new().bg(SURFACE)), area);
}

#[cfg(test)]
mod tests {
    use super::{format_duration, percentage, progress_bar, ratio};

    #[test]
    fn ratios_percentages_and_durations_are_bounded() {
        assert_eq!(ratio(0, 0), 1.0);
        assert_eq!(ratio(5, 10), 0.5);
        assert_eq!(ratio(20, 10), 1.0);
        assert_eq!(percentage(5, 10), 50);
        assert_eq!(percentage(0, 0), 100);

        assert_eq!(format_duration(9), "9s");
        assert_eq!(format_duration(60), "1m 00s");
        assert_eq!(format_duration(125), "2m 05s");
    }

    #[test]
    fn progress_bars_use_the_exact_width() {
        let line = progress_bar(0.5, 10);
        let text: String = line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        assert_eq!(text.chars().count(), 10);
        assert_eq!(
            text.chars().filter(|character| *character == '█').count(),
            5
        );
    }
}
