mod chrome;
mod help;
mod home;
mod layout;
mod palette;
mod presenter;
mod theme;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Block;

use crate::app::interaction::{Overlay, UiState};
use crate::app::model::AppModel;

use self::chrome::render_footer;
use self::theme::{ACCENT, BACKGROUND, TEXT};

/// Renders the full screen: canvas, active overlay or home view, and footer.
///
/// The footer is only drawn when the terminal is tall enough; a zero-sized
/// terminal renders nothing but the background.
pub(super) fn render(frame: &mut Frame<'_>, model: &AppModel, ui: &UiState) {
    let area = frame.area();
    frame.render_widget(
        Block::new().style(Style::new().bg(BACKGROUND).fg(TEXT)),
        area,
    );

    if area.width == 0 || area.height == 0 {
        return;
    }

    let has_footer = area.height >= 3;
    let content = Rect::new(
        area.x,
        area.y,
        area.width,
        area.height.saturating_sub(u16::from(has_footer)),
    );

    match ui.overlay() {
        Some(Overlay::CommandPalette(command_palette)) => {
            palette::render(frame, content, model, command_palette);
        }
        Some(Overlay::Help) => help::render(frame, content, model),
        None => home::render(frame, content, model),
    }

    if has_footer {
        render_footer(
            frame,
            Rect::new(area.x, area.y + area.height - 1, area.width, 1),
            model,
        );
    }
}

/// Renders the accent-colored focus rail on the left edge of `area`.
fn render_focus_rail(frame: &mut Frame<'_>, area: Rect) {
    if area.width > 0 && area.height > 0 {
        frame.render_widget(
            Block::new().style(Style::new().bg(ACCENT)),
            Rect::new(area.x, area.y, 1, area.height),
        );
    }
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::style::Color;

    use super::render;
    use super::theme::{BACKGROUND, HIGHLIGHT, SURFACE};
    use crate::app::action::{DeviceId, KeyInput, UserAction};
    use crate::app::event::AppEvent;
    use crate::app::failure::FailureKind;
    use crate::app::interaction::{UiState, apply_key_input, apply_user_action};
    use crate::app::model::AppModel;
    use crate::app::reducer::update;

    #[test]
    fn basic_screens_render_at_normal_and_small_sizes() {
        let mut browsing = AppModel::new();
        update(&mut browsing, AppEvent::StartupCompleted);

        let mut error = browsing.clone();
        update(&mut error, AppEvent::Failed(FailureKind::Internal));

        let mut shutdown = browsing.clone();
        update(&mut shutdown, AppEvent::ShutdownRequested);

        let cases = [
            (AppModel::new(), "Preparing the terminal"),
            (browsing, "No devices found"),
            (error, "Lanweave encountered"),
            (shutdown, "Shutting down safely"),
        ];

        for (model, expected) in cases {
            for (width, height) in [(80, 24), (32, 12)] {
                let output = render_to_string(&model, width, height);
                assert!(output.contains("lanweave"), "{width}x{height}: {output}");
                assert!(output.contains(expected), "{width}x{height}: {output}");
            }
        }
    }

    #[test]
    fn shell_uses_dark_canvas_centered_surface_and_quiet_footer() {
        let mut model = AppModel::new();
        update(&mut model, AppEvent::StartupCompleted);

        let output = render_to_string(&model, 80, 24);
        let backgrounds = render_backgrounds(&model, 80, 24);

        assert!(output.contains("Browsing for devices"));
        assert!(output.contains("v0.1.0"));
        assert!(backgrounds.contains(&BACKGROUND));
        assert!(backgrounds.contains(&SURFACE));
    }

    #[test]
    fn tiny_terminal_does_not_panic() {
        let output = render_to_string(&AppModel::new(), 1, 1);
        assert!(!output.is_empty());
    }

    #[test]
    fn every_failure_kind_has_safe_user_facing_text() {
        for failure in [
            FailureKind::Startup,
            FailureKind::Connection,
            FailureKind::Pairing,
            FailureKind::Session,
            FailureKind::Transfer,
            FailureKind::Internal,
        ] {
            let mut model = AppModel::new();
            update(&mut model, AppEvent::Failed(failure));
            let output = render_to_string(&model, 80, 24);

            assert!(output.contains("Error"));
            assert!(!output.contains("/home/") && !output.contains('\\'));
        }
    }

    #[test]
    fn command_palette_renders_filtered_disabled_and_empty_results() {
        let mut model = AppModel::new();
        let mut ui = UiState::default();
        update(&mut model, AppEvent::StartupCompleted);
        apply_key_input(&model, &mut ui, KeyInput::Character('/'));

        let browsing = render_with_ui(&model, &ui, 80, 24);
        let backgrounds = render_backgrounds_with_ui(&model, &ui, 80, 24);
        assert!(browsing.contains("/devices"));
        assert!(!browsing.contains("/send"));
        assert!(backgrounds.contains(&HIGHLIGHT));

        apply_key_input(&model, &mut ui, KeyInput::Character('z'));
        let empty = render_with_ui(&model, &ui, 80, 24);
        assert!(empty.contains("No matching commands"));

        let mut session = AppModel::new();
        update(&mut session, AppEvent::StartupCompleted);
        update(
            &mut session,
            AppEvent::User(UserAction::SelectDevice(DeviceId::new(1))),
        );
        update(&mut session, AppEvent::PairingSucceeded);
        update(&mut session, AppEvent::User(UserAction::StartTransfer));
        let mut session_ui = UiState::default();
        apply_key_input(&session, &mut session_ui, KeyInput::Character('/'));
        let busy = render_with_ui(&session, &session_ui, 80, 24);
        assert!(busy.contains("/send"));
        assert!(busy.contains("Transfer already active"));
    }

    #[test]
    fn help_overlay_and_palette_render_on_small_terminals() {
        let model = AppModel::new();
        let mut ui = UiState::default();
        apply_user_action(&mut ui, UserAction::ShowHelp);
        assert!(render_with_ui(&model, &ui, 80, 24).contains("keyboard controls"));

        apply_key_input(&model, &mut ui, KeyInput::Character('/'));
        assert!(!render_with_ui(&model, &ui, 24, 8).is_empty());
        assert!(!render_with_ui(&model, &ui, 1, 1).is_empty());
    }

    fn render_to_string(model: &AppModel, width: u16, height: u16) -> String {
        render_with_ui(model, &UiState::default(), width, height)
    }

    fn render_with_ui(model: &AppModel, ui: &UiState, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, model, ui)).unwrap();
        let buffer = terminal.backend().buffer();

        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn render_backgrounds(model: &AppModel, width: u16, height: u16) -> Vec<Color> {
        render_backgrounds_with_ui(model, &UiState::default(), width, height)
    }

    fn render_backgrounds_with_ui(
        model: &AppModel,
        ui: &UiState,
        width: u16,
        height: u16,
    ) -> Vec<Color> {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, model, ui)).unwrap();
        let buffer = terminal.backend().buffer();
        let mut backgrounds = Vec::with_capacity(usize::from(width) * usize::from(height));

        for y in 0..height {
            for x in 0..width {
                backgrounds.push(buffer[(x, y)].bg);
            }
        }

        backgrounds
    }
}
