use ratatui::layout::Rect;

use super::theme::MAX_SURFACE_WIDTH;

/// Returns `area` with a small horizontal inset and optional top padding.
pub(super) fn inset_surface(area: Rect, top_padding: u16) -> Rect {
    let horizontal = if area.width >= 6 { 3 } else { 1 };
    Rect::new(
        area.x.saturating_add(horizontal),
        area.y.saturating_add(top_padding),
        area.width.saturating_sub(horizontal.saturating_add(1)),
        area.height.saturating_sub(top_padding),
    )
}

/// Returns the surface width for `area`, capped and gutter-adjusted.
pub(super) fn surface_width(area: Rect) -> u16 {
    let gutter = if area.width >= 40 { 4 } else { 0 };
    MAX_SURFACE_WIDTH.min(area.width.saturating_sub(gutter))
}

/// Returns a rectangle of the given size centered inside `area`.
pub(super) fn centered_rect(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    )
}

/// Returns the visible `(start, cursor)` of a list windowed around `cursor`.
///
/// The cursor is clamped to the last row and rests at the bottom edge once the
/// list is longer than `capacity`, matching the device, command, and file
/// review lists.
pub(super) fn scroll_window(len: usize, cursor: usize, capacity: usize) -> (usize, usize) {
    if len == 0 || capacity == 0 {
        return (0, 0);
    }
    let cursor = cursor.min(len - 1);
    (cursor.saturating_add(1).saturating_sub(capacity), cursor)
}

#[cfg(test)]
mod tests {
    use super::scroll_window;

    #[test]
    fn scroll_window_keeps_the_cursor_visible_and_clamps_it() {
        // An empty list or a zero-row window has nothing to show.
        assert_eq!(scroll_window(0, 3, 5), (0, 0));
        assert_eq!(scroll_window(10, 4, 0), (0, 0));

        // A short list starts at the top and keeps the cursor clamped.
        assert_eq!(scroll_window(3, 1, 5), (0, 1));
        assert_eq!(scroll_window(3, 99, 5), (0, 2));

        // The cursor rests at the bottom edge while the window moves down.
        assert_eq!(scroll_window(10, 0, 5), (0, 0));
        assert_eq!(scroll_window(10, 4, 5), (0, 4));
        assert_eq!(scroll_window(10, 5, 5), (1, 5));
        assert_eq!(scroll_window(10, 99, 5), (5, 9));
    }
}
