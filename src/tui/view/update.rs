use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::app::interaction::{DialogFocus, UpdateInput, UpdatePhase};
use crate::app::model::AppModel;
use crate::discovery::escape_display;
use crate::update::INSTALL_COMMAND_LINES;

use super::dialog::{self, Button};
use super::theme::{ACCENT, ERROR, MUTED, TEXT, WARNING};

/// Renders the self-update dialog for the current phase.
///
/// Version strings come from the release tag and error messages are local, but
/// both are escaped before display. The dialog owns every phase: checking,
/// the result of the check, the install, and the restart prompt.
pub(super) fn render(frame: &mut Frame<'_>, area: Rect, model: &AppModel, input: &UpdateInput) {
    let (title_color, body) = body_for(&input.phase, model);
    let buttons = buttons_for(input);
    let hints = hints_for(&input.phase);

    dialog::render_dialog(frame, area, "Update", title_color, body, &buttons, hints);
}

/// Returns the title color and body lines for one phase.
fn body_for(phase: &UpdatePhase, model: &AppModel) -> (ratatui::style::Color, Vec<Line<'static>>) {
    let muted = Style::new().fg(MUTED);
    match phase {
        UpdatePhase::Checking => (
            ACCENT,
            vec![Line::styled("Checking for a newer version…", muted)],
        ),
        UpdatePhase::UpToDate { current } => (
            ACCENT,
            vec![Line::styled(
                format!(
                    "Lanweave v{} is the latest version.",
                    escape_display(current)
                ),
                Style::new().fg(TEXT),
            )],
        ),
        UpdatePhase::NotManaged => {
            let mut body = vec![
                Line::styled(
                    "This copy was not installed by the Lanweave installer,",
                    Style::new().fg(TEXT),
                ),
                Line::styled("so it cannot update itself.", Style::new().fg(TEXT)),
                Line::default(),
                Line::styled("Install the latest release with:", muted),
            ];
            body.extend(
                INSTALL_COMMAND_LINES
                    .iter()
                    .map(|line| Line::styled(*line, Style::new().fg(TEXT))),
            );
            (WARNING, body)
        }
        UpdatePhase::Available { current, new } => {
            let mut body = vec![
                Line::styled(
                    format!(
                        "Lanweave v{} is available (you have v{}).",
                        escape_display(new),
                        escape_display(current)
                    ),
                    Style::new().fg(TEXT),
                ),
                Line::styled("The new version runs after a restart.", muted),
            ];
            body.extend(restart_warning(model));
            (ACCENT, body)
        }
        UpdatePhase::Installing { new } => (
            ACCENT,
            vec![
                Line::styled(
                    format!("Downloading and installing v{}…", escape_display(new)),
                    Style::new().fg(TEXT),
                ),
                Line::styled("This can take a moment.", muted),
            ],
        ),
        UpdatePhase::Installed { new } => {
            let mut body = vec![
                Line::styled(
                    format!("Updated to v{}.", escape_display(new)),
                    Style::new().fg(TEXT),
                ),
                Line::styled("Restart Lanweave to use the new version.", muted),
            ];
            body.extend(restart_warning(model));
            (ACCENT, body)
        }
        UpdatePhase::Failed { message } => (
            ERROR,
            vec![
                Line::styled(escape_display(message), Style::new().fg(ERROR)),
                Line::styled("Nothing was changed.", muted),
            ],
        ),
    }
}

/// Warns that restarting drops a live pairing or session.
fn restart_warning(model: &AppModel) -> Option<Line<'static>> {
    let state = model.state();
    (state.has_session() || state.is_pairing()).then(|| {
        Line::styled(
            "Restarting ends the current session.",
            Style::new().fg(WARNING),
        )
    })
}

/// Returns the dialog buttons for the focused decision, if any.
fn buttons_for(input: &UpdateInput) -> Vec<Button> {
    match &input.phase {
        UpdatePhase::Available { .. } => vec![
            Button::new("Update", input.focus == DialogFocus::Accept),
            Button::new("Cancel", input.focus == DialogFocus::Reject),
        ],
        UpdatePhase::Installed { .. } => vec![
            Button::new("Quit now", input.focus == DialogFocus::Accept),
            Button::new("Later", input.focus == DialogFocus::Reject),
        ],
        UpdatePhase::UpToDate { .. } | UpdatePhase::NotManaged | UpdatePhase::Failed { .. } => {
            vec![Button::new("OK", true)]
        }
        UpdatePhase::Checking | UpdatePhase::Installing { .. } => Vec::new(),
    }
}

/// Returns the keyboard hint line for the current phase.
fn hints_for(phase: &UpdatePhase) -> Option<Line<'static>> {
    let choose = |enter: &'static str, escape: &'static str| {
        Line::from(vec![
            Span::styled("left/right", Style::new().fg(TEXT)),
            Span::styled(" choose   ", Style::new().fg(MUTED)),
            Span::styled("enter", Style::new().fg(TEXT)),
            Span::styled(enter, Style::new().fg(MUTED)),
            Span::styled("   esc", Style::new().fg(TEXT)),
            Span::styled(escape, Style::new().fg(MUTED)),
        ])
    };

    match phase {
        UpdatePhase::Available { .. } => Some(choose(" update   ", " cancel")),
        UpdatePhase::Installed { .. } => Some(choose(" select   ", " later")),
        UpdatePhase::UpToDate { .. } | UpdatePhase::NotManaged | UpdatePhase::Failed { .. } => {
            Some(Line::from(vec![
                Span::styled("enter", Style::new().fg(TEXT)),
                Span::styled(" close", Style::new().fg(MUTED)),
            ]))
        }
        UpdatePhase::Checking | UpdatePhase::Installing { .. } => None,
    }
}
