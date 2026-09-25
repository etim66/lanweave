mod chrome;
mod direct_address;
mod file_selection;
mod help;
mod home;
mod layout;
mod pairing_code;
mod palette;
mod presenter;
mod theme;
mod transfer;
mod transfer_review;

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
        Some(Overlay::DirectAddress(input)) => {
            direct_address::render(frame, content, model, input);
        }
        Some(Overlay::PairingCode(input)) => {
            pairing_code::render(frame, content, model, input);
        }
        Some(Overlay::FileSelection(input)) => {
            file_selection::render(frame, content, model, input);
        }
        Some(Overlay::TransferReview(input)) => {
            transfer_review::render(frame, content, model, input);
        }
        Some(Overlay::Help) => help::render(frame, content, model),
        None => home::render(frame, content, model, ui),
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
    use tokio::time::Instant;

    use super::render;
    use super::theme::{BACKGROUND, HIGHLIGHT, SURFACE};
    use crate::app::action::{DeviceId, KeyInput, PairingPeer, UserAction};
    use crate::app::event::AppEvent;
    use crate::app::failure::FailureKind;
    use crate::app::interaction::{
        FileSelectionInput, Overlay, TransferReviewInput, UiState, apply_key_input,
        apply_user_action, reconcile,
    };
    use crate::app::model::{AppModel, AppState, TransferProgress, TransferProposal};
    use crate::app::reducer::update;
    use crate::discovery::{DiscoveredService, DiscoveryEvent};
    use crate::pairing::PairingCode;
    use crate::protocol::{FileEntry, TransferRequest};
    use crate::transfer::selection::{FileSelection, SelectionIssue};

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
        assert!(browsing.contains("/send"));
        assert!(!browsing.contains("/disconnect"));
        assert!(backgrounds.contains(&HIGHLIGHT));

        apply_key_input(&model, &mut ui, KeyInput::Character('z'));
        let empty = render_with_ui(&model, &ui, 80, 24);
        assert!(empty.contains("No matching commands"));

        let mut session = AppModel::new();
        update(&mut session, AppEvent::StartupCompleted);
        update(
            &mut session,
            AppEvent::Discovery(DiscoveryEvent::Resolved(DiscoveredService::for_test(
                "peer",
                Instant::now(),
            ))),
        );
        update(
            &mut session,
            AppEvent::User(UserAction::SelectDevice(DeviceId::new(1))),
        );
        update(&mut session, AppEvent::PairingSucceeded);
        update(
            &mut session,
            AppEvent::User(UserAction::StartTransfer(FileSelection::default())),
        );
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
        apply_user_action(&model, &mut ui, UserAction::ShowHelp);
        assert!(render_with_ui(&model, &ui, 80, 24).contains("keyboard controls"));

        apply_key_input(&model, &mut ui, KeyInput::Character('/'));
        assert!(!render_with_ui(&model, &ui, 24, 8).is_empty());
        assert!(!render_with_ui(&model, &ui, 1, 1).is_empty());
    }

    #[test]
    fn pairing_prompt_code_display_and_entry_render() {
        // A hostile display name stays escaped in the prompt.
        let mut prompt = AppModel::new();
        update(&mut prompt, AppEvent::StartupCompleted);
        update(
            &mut prompt,
            AppEvent::IncomingPairingRequest(PairingPeer::new(
                Some("peer\nname".to_owned()),
                "192.0.2.10:4242".to_owned(),
            )),
        );
        let output = render_to_string(&prompt, 80, 24);
        assert!(output.contains("Pairing request"));
        assert!(output.contains("untrusted"));
        assert!(output.contains("peer\\u{000A}name"));
        assert!(output.contains("192.0.2.10:4242"));

        // The responder displays the code grouped and never as a wire value.
        let mut display = prompt.clone();
        update(&mut display, AppEvent::User(UserAction::AcceptPairing));
        update(
            &mut display,
            AppEvent::PairingCodeIssued(PairingCode::parse("01234567").unwrap()),
        );
        let output = render_to_string(&display, 80, 24);
        assert!(output.contains("Pairing code"));
        assert!(output.contains("0123 4567"));

        // The initiator's code entry opens with the accepted response.
        let mut entry = browsing_with(&["peer"]);
        let mut ui = UiState::default();
        apply_key_input(&entry, &mut ui, KeyInput::Down);
        assert_eq!(
            apply_key_input(&entry, &mut ui, KeyInput::Enter),
            Some(UserAction::SelectDevice(DeviceId::new(1)))
        );
        update(
            &mut entry,
            AppEvent::User(UserAction::SelectDevice(DeviceId::new(1))),
        );
        update(&mut entry, AppEvent::PairingAccepted);
        reconcile(&entry, &mut ui);
        for character in "1234".chars() {
            apply_key_input(&entry, &mut ui, KeyInput::Character(character));
        }
        let output = render_with_ui(&entry, &ui, 80, 24);
        assert!(output.contains("Enter the pairing code"));
        assert!(output.contains("1234"));
    }

    fn browsing_with(names: &[&str]) -> AppModel {
        let mut model = AppModel::new();
        update(&mut model, AppEvent::StartupCompleted);
        for name in names {
            update(
                &mut model,
                AppEvent::Discovery(DiscoveryEvent::Resolved(DiscoveredService::for_test(
                    name,
                    Instant::now(),
                ))),
            );
        }
        model
    }

    #[test]
    fn device_list_renders_sorted_devices_with_untrusted_note_and_highlight() {
        let model = browsing_with(&["zeta", "alpha"]);
        let output = render_to_string(&model, 80, 24);

        assert!(output.contains("Devices"));
        assert!(output.contains("alpha"));
        assert!(output.contains("zeta"));
        assert!(output.find("alpha").unwrap() < output.find("zeta").unwrap());
        assert!(output.contains(":4242"));
        assert!(output.contains("untrusted"));

        let mut ui = UiState::default();
        apply_key_input(&model, &mut ui, KeyInput::Down);
        let backgrounds = render_backgrounds_with_ui(&model, &ui, 80, 24);
        assert!(backgrounds.contains(&HIGHLIGHT));
    }

    #[test]
    fn browsing_empty_state_shows_searching_text() {
        let model = browsing_with(&[]);
        let output = render_to_string(&model, 80, 24);

        assert!(output.contains("No devices found"));
        assert!(output.contains("Searching the local network"));
        assert!(output.contains("up/down"));
    }

    #[test]
    fn direct_address_card_renders_input_and_validation() {
        let model = browsing_with(&[]);
        let mut ui = UiState::default();
        apply_user_action(&model, &mut ui, UserAction::OpenDirectAddress);
        for character in "peer.local:abc".chars() {
            apply_key_input(&model, &mut ui, KeyInput::Character(character));
        }

        let output = render_with_ui(&model, &ui, 80, 24);
        assert!(output.contains("Connect to a host:port"));
        assert!(output.contains("peer.local:abc"));
        assert!(output.contains("The port must be a number"));
        assert!(output.contains("enter"));
        assert!(output.contains("esc"));
    }

    #[test]
    fn file_review_shows_a_reviewing_note() {
        let review = FileSelectionInput {
            text: "a.txt".to_owned(),
            reviewing: true,
            ..FileSelectionInput::default()
        };
        let ui = UiState::for_test(Overlay::FileSelection(review));
        let session = AppModel::for_test(AppState::SessionIdle);

        let output = render_with_ui(&session, &ui, 80, 24);
        assert!(output.contains("Reviewing pasted paths..."));
    }

    #[test]
    fn idle_session_shows_a_preparation_failure_notice() {
        use crate::transfer::selection::PrepareError;

        let mut model = AppModel::for_test(AppState::SessionIdle);
        update(
            &mut model,
            AppEvent::User(UserAction::StartTransfer(FileSelection::for_test(&[(
                "docs.zip", 1,
            )]))),
        );
        update(
            &mut model,
            AppEvent::PreparationFailed(PrepareError::TooLarge),
        );

        let output = render_to_string(&model, 80, 24);
        assert!(output.contains("more than 100,000 items"));
        assert!(output.contains("Use /send"));
    }

    #[test]
    fn file_review_renders_entries_issues_and_send_state() {
        let review = FileSelectionInput {
            text: String::new(),
            selection: FileSelection::for_test(&[("report.txt", 2048)]),
            issues: vec![SelectionIssue::for_test(
                "missing.txt",
                "the file was not found",
            )],
            selected: Some(0),
            ..FileSelectionInput::default()
        };
        let ui = UiState::for_test(Overlay::FileSelection(review));

        let session = AppModel::for_test(AppState::SessionIdle);
        let output = render_with_ui(&session, &ui, 80, 24);
        assert!(output.contains("Review files"));
        assert!(output.contains("report.txt"));
        assert!(output.contains("2.0 KiB"));
        assert!(output.contains("missing.txt: the file was not found"));
        assert!(output.contains("Press enter to send"));

        // Outside an authorized idle session the same list cannot be sent.
        let browsing = AppModel::for_test(AppState::Browsing);
        let output = render_with_ui(&browsing, &ui, 80, 24);
        assert!(output.contains("Connect and pair"));
        assert!(!output.contains("Press enter to send"));
    }

    #[test]
    fn inbound_transfer_review_renders_manifest_and_destination() {
        let mut model = AppModel::for_test(AppState::SessionIdle);
        update(
            &mut model,
            AppEvent::IncomingTransferRequest(TransferProposal::new(
                &TransferRequest::new(vec![
                    FileEntry::new("report.txt".to_owned(), 2048),
                    FileEntry::new("photo.jpg".to_owned(), 1),
                    FileEntry::folder("docs.zip".to_owned(), 1_234, 42, 987_654),
                ])
                .unwrap(),
                Some("peer".to_owned()),
            )),
        );
        let mut ui = UiState::default();
        reconcile(&model, &mut ui);

        let output = render_with_ui(&model, &ui, 80, 24);
        assert!(output.contains("Incoming files"));
        assert!(output.contains("report.txt"));
        assert!(output.contains("photo.jpg"));
        assert!(output.contains("docs.zip"));
        assert!(output.contains("folder · 42 items"));
        assert!(output.contains("2.0 KiB"));
        assert!(output.contains("From peer"));
        assert!(output.contains("Save to:"));

        // Tiny terminals fall back to a single count line.
        let tiny = render_with_ui(&model, &ui, 30, 4);
        assert!(tiny.contains("3 incoming file(s)"));

        // At six rows the error keeps its reserved row above the destination.
        let mut error_ui = UiState::for_test(Overlay::TransferReview(TransferReviewInput {
            destination: "/tmp".to_owned(),
            error: Some("Enter an existing directory"),
        }));
        reconcile(&model, &mut error_ui);
        let small = render_with_ui(&model, &error_ui, 80, 6);
        assert!(small.contains("Enter an existing directory"));
        assert!(small.contains("Save to:"));
    }

    #[test]
    fn transfer_panels_render_bars_progress_and_destination() {
        let selection = FileSelection::for_test(&[("report.txt", 2_048), ("video.mp4", 4_096)]);
        let mut sending = AppModel::for_test(AppState::SessionIdle);
        update(
            &mut sending,
            AppEvent::User(UserAction::StartTransfer(selection)),
        );
        update(&mut sending, AppEvent::TransferStarted);
        update(
            &mut sending,
            AppEvent::TransferProgress(TransferProgress {
                index: 0,
                files: 2,
                file_size: 2_048,
                transferred: 1_024,
                total_size: 6_144,
                total_transferred: 1_024,
                elapsed: std::time::Duration::from_secs(2),
            }),
        );

        let output = render_to_string(&sending, 80, 24);
        assert!(output.contains("Sending to"));
        assert!(output.contains("file 1 of 2"));
        assert!(output.contains("17%"));
        assert!(output.contains("report.txt"));
        assert!(output.contains("video.mp4"));
        assert!(output.contains('█'));

        let mut receiving = AppModel::for_test(AppState::SessionIdle);
        update(
            &mut receiving,
            AppEvent::IncomingTransferRequest(TransferProposal::new(
                &TransferRequest::new(vec![FileEntry::new("report.txt".to_owned(), 2_048)])
                    .unwrap(),
                Some("peer".to_owned()),
            )),
        );
        update(
            &mut receiving,
            AppEvent::User(UserAction::AcceptTransfer(std::path::PathBuf::from(
                "/incoming",
            ))),
        );
        update(&mut receiving, AppEvent::TransferStarted);

        let output = render_to_string(&receiving, 80, 24);
        assert!(output.contains("Receiving from peer"));
        assert!(output.contains("Saving to: /incoming"));
    }

    #[test]
    fn transfer_summary_renders_files_and_destination() {
        let mut model = AppModel::for_test(AppState::TransferringInbound);
        update(
            &mut model,
            AppEvent::TransferCompleted(crate::app::model::TransferSummary::new(
                crate::app::model::TransferDirection::Received,
                vec![FileEntry::new("report.txt".to_owned(), 2_048)],
                Some(std::path::PathBuf::from("/incoming")),
                Some("peer".to_owned()),
                std::time::Duration::from_secs(3),
            )),
        );

        let output = render_to_string(&model, 80, 24);
        assert!(output.contains("Transfer complete"));
        assert!(output.contains("Received 1 file(s)"));
        assert!(output.contains("from peer"));
        assert!(output.contains("report.txt"));
        assert!(output.contains("Saved to: /incoming"));
        assert!(output.contains("enter"));
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
