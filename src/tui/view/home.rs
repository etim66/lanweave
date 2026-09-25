use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Wrap};

use crate::app::interaction::UiState;
use crate::app::model::{AppModel, AppState, Screen};

use super::dialog::{self, Button};
use super::digits;
use super::layout::{centered_rect, inset_surface, surface_width};
use super::presenter::screen_content;
use super::render_focus_rail;
use super::theme::{ACCENT, BACKGROUND, HIGHLIGHT, MUTED, SURFACE, TEXT, WARNING};
use super::transfer;

/// Renders the home screen: brand, state surface, hints, and tip.
///
/// The browsing screen grows a device list panel; every other screen keeps
/// the centered message surface. Falls back to a single centered line on very
/// small terminals.
pub(super) fn render(frame: &mut Frame<'_>, area: Rect, model: &AppModel, ui: &UiState) {
    if area.width < 12 || area.height < 5 {
        frame.render_widget(
            Paragraph::new(screen_content(model).0)
                .alignment(Alignment::Center)
                .style(Style::new().fg(TEXT).add_modifier(Modifier::BOLD)),
            area,
        );
        return;
    }

    let compact = area.width < 60 || area.height < 16;
    let browsing = model.screen() == Screen::Browsing;
    let transfer =
        model.state().is_transfer_active() || model.state() == AppState::TransferComplete;
    let code_screen = model.state() == AppState::PairingInboundAccepted;
    let brand_height = if surface_width(area) >= 47 && area.height >= 24 {
        WORDMARK_HEIGHT
    } else {
        1
    };
    let base_height: u16 = if browsing || transfer || code_screen {
        18
    } else if compact {
        7
    } else {
        13
    };
    let stack = centered_rect(
        area,
        surface_width(area),
        base_height.saturating_add(brand_height.saturating_sub(1)),
    );

    render_brand(
        frame,
        Rect::new(stack.x, stack.y, stack.width, brand_height),
    );

    let panel_offset: u16 =
        (if compact { 2u16 } else { 3u16 }).saturating_add(brand_height.saturating_sub(1));
    let panel_y = stack.y.saturating_add(panel_offset);
    let stack_bottom = stack.y.saturating_add(stack.height);
    let available_height = stack_bottom.saturating_sub(panel_y);

    let panel_height = if browsing || transfer || code_screen {
        available_height.saturating_sub(if compact { 1 } else { 2 })
    } else {
        (if compact { 3 } else { 5 }).min(available_height)
    };
    if panel_height > 0 {
        render_state_surface(
            frame,
            Rect::new(stack.x, panel_y, stack.width, panel_height),
            model,
            ui,
        );
    }

    let hints_y = panel_y.saturating_add(panel_height).saturating_add(1);
    if hints_y < stack_bottom {
        render_hints(frame, Rect::new(stack.x, hints_y, stack.width, 1), model);
    }

    let tip_y = hints_y.saturating_add(2);
    if !compact && !browsing && tip_y < stack_bottom {
        render_tip(frame, Rect::new(stack.x, tip_y, stack.width, 1));
    }
}

/// Renders the "lanweave" brand: a large block wordmark on roomy terminals.
///
/// The wordmark is two-tone like the compact one and falls back to a single
/// centered line when the terminal is too narrow.
fn render_brand(frame: &mut Frame<'_>, area: Rect) {
    if let Some(lines) = wordmark_lines(area.width, area.height) {
        frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), area);
        return;
    }

    let brand = Line::from(vec![
        Span::styled("lan", Style::new().fg(MUTED).add_modifier(Modifier::BOLD)),
        Span::styled("weave", Style::new().fg(TEXT).add_modifier(Modifier::BOLD)),
    ]);
    frame.render_widget(Paragraph::new(brand).alignment(Alignment::Center), area);
}

/// Width of one large wordmark glyph in columns.
const WORDMARK_GLYPH_WIDTH: u16 = 5;
/// Height of the large wordmark in rows.
const WORDMARK_HEIGHT: u16 = 4;

/// Returns the large two-tone wordmark when `area` can hold it.
///
/// The word is rendered as "LAN" in the muted tone and "WEAVE" in the bright
/// tone, matching the compact brand.
fn wordmark_lines(width: u16, height: u16) -> Option<Vec<Line<'static>>> {
    const WORD: [char; 8] = ['L', 'A', 'N', 'W', 'E', 'A', 'V', 'E'];
    const GAP: u16 = 1;

    let required =
        WORDMARK_GLYPH_WIDTH * u16::try_from(WORD.len()).ok()? + GAP * (WORD.len() as u16 - 1);
    if width < required || height < WORDMARK_HEIGHT {
        return None;
    }

    let mut lines = Vec::with_capacity(usize::from(WORDMARK_HEIGHT));
    for row in 0..usize::from(WORDMARK_HEIGHT) {
        let mut spans = Vec::new();
        for (index, character) in WORD.iter().enumerate() {
            let glyph = glyph_row(*character, row)?;
            let style = if index < 3 {
                Style::new().fg(MUTED).add_modifier(Modifier::BOLD)
            } else {
                Style::new().fg(TEXT).add_modifier(Modifier::BOLD)
            };
            if index > 0 {
                spans.push(Span::raw(" "));
            }
            spans.push(Span::styled(glyph, style));
        }
        lines.push(Line::from(spans));
    }
    Some(lines)
}

/// Returns one row of a 5x4 block glyph, or `None` for an unknown character.
fn glyph_row(character: char, row: usize) -> Option<String> {
    let bitmap: [u8; 4] = match character {
        'L' => [0b10000, 0b10000, 0b10000, 0b11110],
        'A' => [0b01110, 0b10001, 0b11111, 0b10001],
        'N' => [0b10001, 0b11001, 0b10101, 0b10011],
        'W' => [0b10001, 0b10001, 0b10101, 0b01010],
        'E' => [0b11111, 0b10000, 0b11110, 0b11111],
        'V' => [0b10001, 0b10001, 0b01010, 0b00100],
        _ => return None,
    };
    let bits = bitmap.get(row)?;
    Some(
        (0..WORDMARK_GLYPH_WIDTH)
            .map(|column| {
                let shift = WORDMARK_GLYPH_WIDTH - 1 - column;
                if bits & (1 << shift) != 0 { '█' } else { ' ' }
            })
            .collect(),
    )
}

/// Renders the centered surface: device list while browsing, message otherwise.
fn render_state_surface(frame: &mut Frame<'_>, area: Rect, model: &AppModel, ui: &UiState) {
    frame.render_widget(Block::new().style(Style::new().bg(SURFACE)), area);
    render_focus_rail(frame, area);

    if area.width < 2 || area.height == 0 {
        return;
    }

    if model.state().is_transfer_active() {
        transfer::render_panel(frame, area, model);
    } else if model.state() == AppState::TransferComplete {
        transfer::render_summary_panel(frame, area, model);
    } else if model.state() == AppState::PairingInboundAccepted {
        render_pairing_code_panel(frame, area, model);
    } else if model.screen() == Screen::Browsing {
        render_device_panel(frame, area, model, ui);
    } else {
        render_message_panel(frame, area, model);
    }
}

/// Renders the responder's one-time code in the large digit font.
///
/// The code exists only after local acceptance and never leaves this device;
/// it is shown large so the receiver can share it with the sender. The cancel
/// button ends the provisional connection.
fn render_pairing_code_panel(frame: &mut Frame<'_>, area: Rect, model: &AppModel) {
    let inner = inset_surface(area, u16::from(area.height >= 4));
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    render_panel_line(
        frame,
        Rect::new(inner.x, inner.y, inner.width, 1),
        Line::styled(
            "Share this code",
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
    );

    let Some(code) = model.pairing_code() else {
        return;
    };
    let digits: Vec<char> = code.grouped().chars().collect();
    let glyph_width = u16::try_from(digits::width(digits.len())).unwrap_or(u16::MAX);
    let first_row = inner.y.saturating_add(2);
    if glyph_width > inner.width {
        render_panel_line(
            frame,
            Rect::new(inner.x, first_row, inner.width, 1),
            Line::styled(
                code.grouped(),
                Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
        );
    } else {
        for row_index in 0..digits::ROWS {
            let row_y = first_row.saturating_add(u16::try_from(row_index).unwrap_or(u16::MAX));
            if row_y >= inner.y + inner.height {
                break;
            }
            frame.render_widget(
                Paragraph::new(Line::styled(
                    digits::row(&digits, row_index),
                    Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
                ))
                .alignment(Alignment::Center)
                .style(Style::new().bg(SURFACE)),
                Rect::new(inner.x, row_y, inner.width, 1),
            );
        }
    }

    // The bottom rows carry the sharing note and the cancel button.
    let button_y = inner.y + inner.height - 1;
    if inner.height >= 10 {
        render_panel_line(
            frame,
            Rect::new(inner.x, button_y - 2, inner.width, 1),
            Line::styled(
                "Give this code to the sender to authorize the session.",
                Style::new().fg(TEXT),
            ),
        );
        render_panel_line(
            frame,
            Rect::new(inner.x, button_y - 1, inner.width, 1),
            Line::styled(
                "It expires after about two minutes and is never sent over the connection.",
                Style::new().fg(MUTED),
            ),
        );
    }
    if inner.height >= 8 {
        dialog::render_buttons(
            frame,
            Rect::new(inner.x, button_y, inner.width, 1),
            &[Button::new("Cancel pairing", true)],
        );
    }
}

/// Renders the title and message for a non-browsing screen.
///
/// The message may contain newlines; each line is rendered separately so a
/// multi-sentence prompt never runs together on one row. Waiting screens with
/// one available action show it as a highlighted button.
fn render_message_panel(frame: &mut Frame<'_>, area: Rect, model: &AppModel) {
    let (title, message, color) = screen_content(model);
    let top_padding = u16::from(area.height >= 4);
    let inner = inset_surface(area, top_padding);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let action = action_label(model.state());
    let button_rows = if action.is_some() && inner.height >= 4 {
        2
    } else {
        0
    };
    let body_height = inner.height.saturating_sub(button_rows);
    let mut lines = vec![Line::styled(
        title,
        Style::new().fg(color).add_modifier(Modifier::BOLD),
    )];
    lines.extend(
        message
            .lines()
            .map(|line| Line::styled(line.to_owned(), Style::new().fg(MUTED))),
    );
    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::new().bg(SURFACE))
            .wrap(Wrap { trim: true }),
        Rect::new(inner.x, inner.y, inner.width, body_height),
    );

    if let (Some(label), true) = (action, button_rows > 0) {
        dialog::render_buttons(
            frame,
            Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1),
            &[Button::new(label, true)],
        );
    }
}

/// Returns the single action a waiting screen offers, if any.
fn action_label(state: AppState) -> Option<&'static str> {
    match state {
        AppState::PairingOutbound => Some("Cancel request"),
        AppState::PairingConfirming => Some("Cancel pairing"),
        AppState::OutboundProposal => Some("Cancel transfer"),
        _ => None,
    }
}

/// Renders the live device list: title, rows, and the untrusted note.
///
/// Rows are scrolled around the selected device so the highlight never leaves
/// the panel. The empty state explains that discovery is still searching.
fn render_device_panel(frame: &mut Frame<'_>, area: Rect, model: &AppModel, ui: &UiState) {
    let inner = inset_surface(area, u16::from(area.height >= 4));
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let candidates = model.sorted_candidates();
    let (title, title_color) = if candidates.is_empty() {
        ("No devices found", WARNING)
    } else {
        ("Devices", ACCENT)
    };
    render_panel_line(
        frame,
        Rect::new(inner.x, inner.y, inner.width, 1),
        Line::styled(
            title,
            Style::new().fg(title_color).add_modifier(Modifier::BOLD),
        ),
    );

    if candidates.is_empty() {
        render_panel_line(
            frame,
            Rect::new(
                inner.x,
                inner.y + 1,
                inner.width,
                inner.height.saturating_sub(1),
            ),
            Line::styled("Searching the local network...", Style::new().fg(MUTED)),
        );
        return;
    }

    let note_height = u16::from(inner.height >= 3);
    let row_capacity = inner.height.saturating_sub(2).saturating_sub(note_height);
    if row_capacity == 0 {
        return;
    }

    let selected = ui.device_selection();
    let selected_index = selected.and_then(|selected| {
        candidates
            .iter()
            .position(|candidate| candidate.id() == selected)
    });
    let start = selected_index
        .unwrap_or(0)
        .saturating_add(1)
        .saturating_sub(usize::from(row_capacity));

    for (offset, candidate) in candidates
        .iter()
        .skip(start)
        .take(usize::from(row_capacity))
        .enumerate()
    {
        render_device_row(
            frame,
            Rect::new(
                inner.x,
                inner.y + 1 + u16::try_from(offset).unwrap_or(u16::MAX),
                inner.width,
                1,
            ),
            candidate,
            selected == Some(candidate.id()),
        );
    }

    if note_height > 0 {
        render_panel_line(
            frame,
            Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1),
            Line::from(vec![
                Span::styled("?", Style::new().fg(WARNING).add_modifier(Modifier::BOLD)),
                Span::styled(
                    " untrusted until pairing confirms the device",
                    Style::new().fg(MUTED),
                ),
            ]),
        );
    }
}

/// Renders one device row with the selection highlight.
fn render_device_row(
    frame: &mut Frame<'_>,
    area: Rect,
    candidate: &crate::discovery::Candidate,
    is_selected: bool,
) {
    let row_style = if is_selected {
        Style::new().bg(HIGHLIGHT).fg(BACKGROUND)
    } else {
        Style::new().bg(SURFACE).fg(TEXT)
    };
    let name_style = row_style.add_modifier(Modifier::BOLD);

    let line = if area.width >= 40 {
        Line::from(vec![
            Span::styled(format!("  {}", candidate.display_name()), name_style),
            Span::styled(
                format!("  {}:{}", candidate.host(), candidate.port()),
                row_style,
            ),
        ])
    } else {
        Line::from(Span::styled(
            format!("  {}", candidate.display_name()),
            name_style,
        ))
    };
    frame.render_widget(Paragraph::new(line).style(row_style), area);
}

/// Renders one background-filled line inside the device panel.
fn render_panel_line(frame: &mut Frame<'_>, area: Rect, line: Line<'_>) {
    frame.render_widget(Paragraph::new(line).style(Style::new().bg(SURFACE)), area);
}

/// Renders the one-line keyboard hints below the state surface.
fn render_hints(frame: &mut Frame<'_>, area: Rect, model: &AppModel) {
    let spans = match model.state() {
        AppState::Home => vec![
            Span::styled("/devices", Style::new().fg(TEXT)),
            Span::styled(" devices   ", Style::new().fg(MUTED)),
            Span::styled("/send", Style::new().fg(TEXT)),
            Span::styled(" send files   ", Style::new().fg(MUTED)),
            Span::styled("/help", Style::new().fg(TEXT)),
            Span::styled(" help   ", Style::new().fg(MUTED)),
            Span::styled("q", Style::new().fg(TEXT)),
            Span::styled(" quit", Style::new().fg(MUTED)),
        ],
        AppState::Browsing => vec![
            Span::styled("up/down", Style::new().fg(TEXT)),
            Span::styled(" select   ", Style::new().fg(MUTED)),
            Span::styled("enter", Style::new().fg(TEXT)),
            Span::styled(" connect   ", Style::new().fg(MUTED)),
            Span::styled("/", Style::new().fg(TEXT)),
            Span::styled(" commands   ", Style::new().fg(MUTED)),
            Span::styled("q", Style::new().fg(TEXT)),
            Span::styled(" quit", Style::new().fg(MUTED)),
        ],
        AppState::PairingInbound => vec![
            Span::styled("left/right", Style::new().fg(TEXT)),
            Span::styled(" choose   ", Style::new().fg(MUTED)),
            Span::styled("enter", Style::new().fg(TEXT)),
            Span::styled(" select   ", Style::new().fg(MUTED)),
            Span::styled("esc", Style::new().fg(TEXT)),
            Span::styled(" reject   ", Style::new().fg(MUTED)),
            Span::styled("q", Style::new().fg(TEXT)),
            Span::styled(" quit", Style::new().fg(MUTED)),
        ],
        AppState::PairingOutbound => vec![
            Span::styled("esc", Style::new().fg(TEXT)),
            Span::styled(" cancel request   ", Style::new().fg(MUTED)),
            Span::styled("/", Style::new().fg(TEXT)),
            Span::styled(" commands   ", Style::new().fg(MUTED)),
            Span::styled("q", Style::new().fg(TEXT)),
            Span::styled(" quit", Style::new().fg(MUTED)),
        ],
        AppState::PairingInboundAccepted => vec![
            Span::styled("esc", Style::new().fg(TEXT)),
            Span::styled(" cancel pairing   ", Style::new().fg(MUTED)),
            Span::styled("/", Style::new().fg(TEXT)),
            Span::styled(" commands   ", Style::new().fg(MUTED)),
            Span::styled("q", Style::new().fg(TEXT)),
            Span::styled(" quit", Style::new().fg(MUTED)),
        ],
        AppState::PairingOutboundAccepted => vec![
            Span::styled("0-9", Style::new().fg(TEXT)),
            Span::styled(" type   ", Style::new().fg(MUTED)),
            Span::styled("enter", Style::new().fg(TEXT)),
            Span::styled(" submit   ", Style::new().fg(MUTED)),
            Span::styled("esc", Style::new().fg(TEXT)),
            Span::styled(" cancel", Style::new().fg(MUTED)),
        ],
        AppState::PairingConfirming => vec![
            Span::styled("esc", Style::new().fg(TEXT)),
            Span::styled(" cancel pairing   ", Style::new().fg(MUTED)),
            Span::styled("q", Style::new().fg(TEXT)),
            Span::styled(" quit", Style::new().fg(MUTED)),
        ],
        AppState::TransferComplete => vec![
            Span::styled("enter", Style::new().fg(TEXT)),
            Span::styled(" dismiss   ", Style::new().fg(MUTED)),
            Span::styled("/", Style::new().fg(TEXT)),
            Span::styled(" commands   ", Style::new().fg(MUTED)),
            Span::styled("q", Style::new().fg(TEXT)),
            Span::styled(" quit", Style::new().fg(MUTED)),
        ],
        AppState::OutboundProposal
        | AppState::TransferringOutbound
        | AppState::TransferringInbound => vec![
            Span::styled("esc", Style::new().fg(TEXT)),
            Span::styled(" cancel transfer   ", Style::new().fg(MUTED)),
            Span::styled("/", Style::new().fg(TEXT)),
            Span::styled(" commands   ", Style::new().fg(MUTED)),
            Span::styled("q", Style::new().fg(TEXT)),
            Span::styled(" quit", Style::new().fg(MUTED)),
        ],
        _ => vec![
            Span::styled("/", Style::new().fg(TEXT)),
            Span::styled(" commands   ", Style::new().fg(MUTED)),
            Span::styled("q", Style::new().fg(TEXT)),
            Span::styled(" quit   ", Style::new().fg(MUTED)),
            Span::styled("ctrl+c", Style::new().fg(TEXT)),
            Span::styled(" quit", Style::new().fg(MUTED)),
        ],
    };
    frame.render_widget(
        Paragraph::new(Line::from(spans)).alignment(Alignment::Left),
        area,
    );
}

/// Renders the "Tip" line shown on roomy terminals.
fn render_tip(frame: &mut Frame<'_>, area: Rect) {
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "Tip ",
                Style::new().fg(WARNING).add_modifier(Modifier::BOLD),
            ),
            Span::styled("Open commands with ", Style::new().fg(MUTED)),
            Span::styled("/", Style::new().fg(TEXT)),
        ]))
        .alignment(Alignment::Center),
        area,
    );
}
