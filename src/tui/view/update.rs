use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

use crate::app::interaction::{DialogFocus, UpdateInput, UpdatePhase};
use crate::app::model::AppModel;
use crate::discovery::escape_display;
use crate::update::INSTALL_COMMAND_LINES;

use super::dialog::{self, Button};
use super::theme::{ACCENT, ERROR, MUTED, SUCCESS, TEXT, WARNING};

/// Renders the self-update dialog for the current phase.
///
/// Version strings come from the release tag and error messages are local, but
/// both are escaped before display. The dialog owns every phase: checking,
/// the result of the check, the install, and the restart prompt.
pub(super) fn render(frame: &mut Frame<'_>, area: Rect, model: &AppModel, input: &UpdateInput) {
    let (title, title_color) = title_for(&input.phase);
    let body = body_for(&input.phase, model);
    let buttons = buttons_for(input);
    let hints = hints_for(&input.phase);
    let mut spec = dialog::DialogSpec::new(title, title_color).buttons(&buttons);
    if let Some(hint) = header_hint_for(&input.phase) {
        spec = spec.header_hint(hint);
    }
    if let Some(hints) = hints {
        spec = spec.footer_hint(hints);
    }

    dialog::render_dialog(frame, area, spec, body);
}

/// Returns the dialog title and title color for one phase.
fn title_for(phase: &UpdatePhase) -> (&'static str, Color) {
    match phase {
        UpdatePhase::Checking | UpdatePhase::UpToDate { .. } => ("Update", ACCENT),
        UpdatePhase::NotManaged => ("Update", WARNING),
        UpdatePhase::Available { .. } => ("Update available", ACCENT),
        UpdatePhase::Installing { .. } => ("Updating Lanweave", ACCENT),
        UpdatePhase::Installed { .. } => ("Update complete", SUCCESS),
        UpdatePhase::Failed { .. } => ("Update failed", ERROR),
    }
}

/// Returns the body lines for one phase.
fn body_for(phase: &UpdatePhase, model: &AppModel) -> Vec<Line<'static>> {
    let muted = Style::new().fg(MUTED);
    match phase {
        UpdatePhase::Checking => vec![Line::styled("Checking for a newer version…", muted)],
        UpdatePhase::UpToDate { current } => vec![Line::styled(
            format!(
                "Lanweave v{} is the latest version.",
                escape_display(current)
            ),
            Style::new().fg(TEXT),
        )],
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
            body
        }
        UpdatePhase::Available { current, new } => {
            let mut body = vec![
                Line::styled(
                    format!("Lanweave v{} is available.", escape_display(new)),
                    Style::new().fg(TEXT),
                ),
                Line::styled(format!("You have v{}.", escape_display(current)), muted),
                Line::default(),
                Line::styled("The new version runs after a restart.", muted),
            ];
            body.extend(restart_warning(model));
            body
        }
        UpdatePhase::Installing { new } => vec![
            Line::styled(
                format!("Downloading and installing v{}…", escape_display(new)),
                Style::new().fg(TEXT),
            ),
            Line::styled("This can take a moment.", muted),
        ],
        UpdatePhase::Installed { new } => {
            let mut body = vec![
                Line::styled(
                    format!("Lanweave v{} is installed.", escape_display(new)),
                    Style::new().fg(TEXT),
                ),
                Line::styled("Restart Lanweave to use the new version.", muted),
            ];
            body.extend(restart_warning(model));
            body
        }
        UpdatePhase::Failed { message } => vec![
            Line::styled(escape_display(message), Style::new().fg(ERROR)),
            Line::styled("Nothing was changed.", muted),
        ],
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

/// Returns the top-right dismiss hint for phases that accept Escape.
fn header_hint_for(phase: &UpdatePhase) -> Option<&'static str> {
    match phase {
        UpdatePhase::Checking | UpdatePhase::Installing { .. } => None,
        _ => Some("esc"),
    }
}

/// Returns the keyboard hint line for the current phase.
fn hints_for(phase: &UpdatePhase) -> Option<Line<'static>> {
    let choose = || {
        Line::from(vec![
            Span::styled("left/right", Style::new().fg(TEXT)),
            Span::styled(" choose", Style::new().fg(MUTED)),
        ])
    };

    match phase {
        UpdatePhase::Available { .. } | UpdatePhase::Installed { .. } => Some(choose()),
        UpdatePhase::UpToDate { .. } | UpdatePhase::NotManaged | UpdatePhase::Failed { .. } => {
            Some(Line::from(vec![
                Span::styled("enter", Style::new().fg(TEXT)),
                Span::styled(" close", Style::new().fg(MUTED)),
            ]))
        }
        UpdatePhase::Checking | UpdatePhase::Installing { .. } => None,
    }
}
