use ratatui::style::Color;

/// Terminal background color.
pub(super) const BACKGROUND: Color = Color::Rgb(8, 8, 8);
/// Surface color for cards and panels.
pub(super) const SURFACE: Color = Color::Rgb(30, 30, 30);
/// Primary foreground text color.
pub(super) const TEXT: Color = Color::Rgb(220, 220, 220);
/// Secondary text color for hints and metadata.
pub(super) const MUTED: Color = Color::Rgb(128, 128, 128);
/// Accent color for titles and interactive elements.
pub(super) const ACCENT: Color = Color::Rgb(232, 158, 100);
/// Highlight color for the selected row.
pub(super) const HIGHLIGHT: Color = Color::Rgb(247, 181, 128);
/// Success color for completed files and transfers.
pub(super) const SUCCESS: Color = Color::Rgb(137, 192, 108);
/// Unfilled part of a progress bar.
pub(super) const PROGRESS_TRACK: Color = Color::Rgb(64, 64, 64);
/// Warning color for transitional messages.
pub(super) const WARNING: Color = Color::Rgb(238, 171, 74);
/// Error color for failure messages.
pub(super) const ERROR: Color = Color::Rgb(241, 112, 122);
/// Maximum width of a centered surface.
pub(super) const MAX_SURFACE_WIDTH: u16 = 76;
