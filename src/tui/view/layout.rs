use ratatui::layout::Rect;

use super::theme::MAX_SURFACE_WIDTH;

pub(super) fn inset_surface(area: Rect, top_padding: u16) -> Rect {
    let horizontal = if area.width >= 6 { 3 } else { 1 };
    Rect::new(
        area.x.saturating_add(horizontal),
        area.y.saturating_add(top_padding),
        area.width.saturating_sub(horizontal.saturating_add(1)),
        area.height.saturating_sub(top_padding),
    )
}

pub(super) fn surface_width(area: Rect) -> u16 {
    let gutter = if area.width >= 40 { 4 } else { 0 };
    MAX_SURFACE_WIDTH.min(area.width.saturating_sub(gutter))
}

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
