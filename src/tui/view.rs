use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::app::{AppModel, AppState, FailureKind, Screen};

pub(super) fn render(frame: &mut Frame<'_>, model: &AppModel) {
    let [header, status, content, footer] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Min(1),
        Constraint::Length(3),
    ])
    .areas(frame.area());

    frame.render_widget(
        Paragraph::new("LANWEAVE")
            .alignment(Alignment::Center)
            .style(Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD))
            .block(Block::new().borders(Borders::ALL)),
        header,
    );

    frame.render_widget(
        Paragraph::new(Line::from(status_text(model.state())))
            .block(Block::new().title(" Status ").borders(Borders::ALL)),
        status,
    );

    let (title, message, color) = screen_content(model);
    frame.render_widget(
        Paragraph::new(message)
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true })
            .style(Style::new().fg(color))
            .block(Block::new().title(title).borders(Borders::ALL)),
        content,
    );

    frame.render_widget(
        Paragraph::new("q quit  |  Ctrl+C quit")
            .alignment(Alignment::Center)
            .style(Style::new().fg(Color::DarkGray))
            .block(Block::new().borders(Borders::ALL)),
        footer,
    );
}

fn status_text(state: AppState) -> &'static str {
    match state {
        AppState::Starting => "Starting",
        AppState::Browsing => "Browsing for devices",
        AppState::PairingOutbound
        | AppState::PairingInbound
        | AppState::PairingInboundAccepted
        | AppState::ClosingPairing => "Pairing",
        AppState::SessionIdle | AppState::ClosingSession => "Session active",
        AppState::OutboundProposal
        | AppState::InboundProposal
        | AppState::InboundProposalAccepted
        | AppState::TransferringOutbound
        | AppState::TransferringInbound => "Transfer",
        AppState::Error(_) => "Error",
        AppState::ShuttingDown => "Shutting down",
    }
}

fn screen_content(model: &AppModel) -> (&'static str, &'static str, Color) {
    match model.screen() {
        Screen::Starting => (" Starting ", "Preparing the terminal...", Color::Yellow),
        Screen::Browsing => (
            " Devices ",
            "No devices found. Discovery will appear here when it is available.",
            Color::White,
        ),
        Screen::Error => (" Error ", failure_message(model.state()), Color::LightRed),
        Screen::Shutdown => (" Shutdown ", "Closing Lanweave safely...", Color::Yellow),
        Screen::Pairing => (
            " Pairing ",
            "Pairing controls are not available in this build.",
            Color::White,
        ),
        Screen::Session => (
            " Session ",
            "Session controls are not available in this build.",
            Color::White,
        ),
        Screen::Transfer => (
            " Transfer ",
            "Transfer controls are not available in this build.",
            Color::White,
        ),
    }
}

fn failure_message(state: AppState) -> &'static str {
    match state {
        AppState::Error(FailureKind::Startup) => "Lanweave could not start.",
        AppState::Error(FailureKind::Connection) => "The connection failed.",
        AppState::Error(FailureKind::Pairing) => "Pairing failed.",
        AppState::Error(FailureKind::Session) => "The session failed.",
        AppState::Error(FailureKind::Transfer) => "The transfer failed.",
        AppState::Error(FailureKind::Internal) => "Lanweave encountered an internal error.",
        _ => "Lanweave encountered an error.",
    }
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::render;
    use crate::app::{AppEvent, AppModel, FailureKind, update};

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
            (shutdown, "Closing Lanweave safely"),
        ];

        for (model, expected) in cases {
            for (width, height) in [(80, 24), (32, 12)] {
                let output = render_to_string(&model, width, height);
                assert!(output.contains("LANWEAVE"), "{width}x{height}: {output}");
                assert!(output.contains(expected), "{width}x{height}: {output}");
            }
        }
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
            assert!(!output.contains('/') && !output.contains('\\'));
        }
    }

    fn render_to_string(model: &AppModel, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, model)).unwrap();
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
}
