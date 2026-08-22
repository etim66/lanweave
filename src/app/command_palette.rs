//! State-aware slash command registry and selection behavior.

use super::action::UserAction;
use super::model::AppCapabilities;

/// Maximum number of characters a palette query may hold.
pub(crate) const MAX_COMMAND_QUERY_CHARS: usize = 64;

/// Identifies a slash command independently of its current availability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum CommandId {
    Help,
    Devices,
    Connect,
    Send,
    Disconnect,
    Quit,
}

/// Whether a command may run, and why not when it may not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommandAvailability {
    Enabled,
    Disabled(&'static str),
    Hidden,
}

/// Static description of one slash command.
#[derive(Debug, Clone)]
pub(crate) struct CommandSpec {
    pub(crate) id: CommandId,
    pub(crate) name: &'static str,
    pub(crate) description: &'static str,
    pub(crate) availability: fn(AppCapabilities) -> CommandAvailability,
    pub(crate) action: UserAction,
}

/// The complete slash command table, in display order.
const COMMANDS: [CommandSpec; 6] = [
    CommandSpec {
        id: CommandId::Help,
        name: "/help",
        description: "Show commands and keyboard controls",
        availability: always_available,
        action: UserAction::ShowHelp,
    },
    CommandSpec {
        id: CommandId::Devices,
        name: "/devices",
        description: "Show devices currently running Lanweave",
        availability: devices_availability,
        action: UserAction::ShowDevices,
    },
    CommandSpec {
        id: CommandId::Connect,
        name: "/connect",
        description: "Connect to a host:port directly",
        availability: devices_availability,
        action: UserAction::OpenDirectAddress,
    },
    CommandSpec {
        id: CommandId::Send,
        name: "/send",
        description: "Send reviewed files",
        availability: send_availability,
        action: UserAction::StartTransfer,
    },
    CommandSpec {
        id: CommandId::Disconnect,
        name: "/disconnect",
        description: "Close the current connection",
        availability: disconnect_availability,
        action: UserAction::Disconnect,
    },
    CommandSpec {
        id: CommandId::Quit,
        name: "/quit",
        description: "Close Lanweave",
        availability: always_available,
        action: UserAction::Quit,
    },
];

/// Returns the full slash command table.
pub(crate) fn registry() -> &'static [CommandSpec] {
    &COMMANDS
}

/// Returns the commands visible for `capabilities`, filtered by `query`.
///
/// A leading `/` in the query is ignored, and matching is case-insensitive
/// against both the command name and its description.
pub(crate) fn visible_commands(
    capabilities: AppCapabilities,
    query: &str,
) -> Vec<&'static CommandSpec> {
    let query = query.trim_start_matches('/').to_ascii_lowercase();

    COMMANDS
        .iter()
        .filter(|command| (command.availability)(capabilities) != CommandAvailability::Hidden)
        .filter(|command| {
            query.is_empty()
                || command.name[1..].to_ascii_lowercase().contains(&query)
                || command.description.to_ascii_lowercase().contains(&query)
        })
        .collect()
}

/// Returns the first visible command for `query`, if any.
pub(crate) fn first_visible(capabilities: AppCapabilities, query: &str) -> Option<CommandId> {
    visible_commands(capabilities, query)
        .first()
        .map(|command| command.id)
}

/// Moves the palette selection by one command, wrapping at the ends.
pub(crate) fn move_selection(
    capabilities: AppCapabilities,
    query: &str,
    selected: Option<CommandId>,
    forward: bool,
) -> Option<CommandId> {
    let commands = visible_commands(capabilities, query);
    if commands.is_empty() {
        return None;
    }

    let current = selected.and_then(|id| commands.iter().position(|command| command.id == id));
    let next = match (current, forward) {
        (Some(index), true) => (index + 1) % commands.len(),
        (Some(0), false) | (None, false) => commands.len() - 1,
        (None, true) => 0,
        (Some(index), false) => index - 1,
    };
    Some(commands[next].id)
}

/// Keeps `selected` valid for the current query, falling back to the first command.
pub(crate) fn reconcile_selection(
    capabilities: AppCapabilities,
    query: &str,
    selected: Option<CommandId>,
) -> Option<CommandId> {
    let commands = visible_commands(capabilities, query);
    selected
        .filter(|id| commands.iter().any(|command| command.id == *id))
        .or_else(|| commands.first().map(|command| command.id))
}

/// Resolves the selected command to its action, if it is currently enabled.
pub(crate) fn resolve(
    capabilities: AppCapabilities,
    query: &str,
    selected: Option<CommandId>,
) -> Option<UserAction> {
    let selected = selected?;
    visible_commands(capabilities, query)
        .into_iter()
        .find(|command| command.id == selected)
        .filter(|command| (command.availability)(capabilities) == CommandAvailability::Enabled)
        .map(|command| command.action.clone())
}

/// Availability for commands that run whenever the app accepts commands.
fn always_available(capabilities: AppCapabilities) -> CommandAvailability {
    if capabilities.accepts_commands {
        CommandAvailability::Enabled
    } else {
        CommandAvailability::Hidden
    }
}

/// Availability for the devices command, which needs the browsing screen.
fn devices_availability(capabilities: AppCapabilities) -> CommandAvailability {
    if capabilities.can_show_devices {
        CommandAvailability::Enabled
    } else {
        CommandAvailability::Hidden
    }
}

/// Availability for the send command, which needs an idle session.
fn send_availability(capabilities: AppCapabilities) -> CommandAvailability {
    if capabilities.can_start_transfer {
        CommandAvailability::Enabled
    } else if capabilities.session_closing {
        CommandAvailability::Disabled("The session is closing")
    } else if capabilities.transfer_unavailable {
        CommandAvailability::Disabled("Transfer already active")
    } else {
        CommandAvailability::Hidden
    }
}

/// Availability for the disconnect command, which needs an active connection.
fn disconnect_availability(capabilities: AppCapabilities) -> CommandAvailability {
    if capabilities.can_disconnect {
        CommandAvailability::Enabled
    } else if capabilities.disconnecting {
        CommandAvailability::Disabled("Disconnect already in progress")
    } else {
        CommandAvailability::Hidden
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{
        CommandAvailability, CommandId, first_visible, move_selection, reconcile_selection,
        registry, resolve, visible_commands,
    };
    use crate::app::action::UserAction;
    use crate::app::model::{AppModel, AppState};

    #[test]
    fn registry_has_unique_commands_and_expected_actions() {
        let commands = registry();
        let names = commands
            .iter()
            .map(|command| command.name)
            .collect::<HashSet<_>>();

        assert_eq!(commands.len(), 6);
        assert_eq!(names.len(), commands.len());
        assert!(commands.iter().all(|command| command.name.starts_with('/')));
        assert_eq!(commands[0].action, UserAction::ShowHelp);
        assert_eq!(commands[1].action, UserAction::ShowDevices);
        assert_eq!(commands[2].action, UserAction::OpenDirectAddress);
        assert_eq!(commands[3].action, UserAction::StartTransfer);
        assert_eq!(commands[4].action, UserAction::Disconnect);
        assert_eq!(commands[5].action, UserAction::Quit);
    }

    #[test]
    fn availability_matches_application_state() {
        for state in AppState::ALL {
            let capabilities = AppModel::for_test(state).capabilities();
            let availability = |id| {
                let command = registry().iter().find(|command| command.id == id).unwrap();
                (command.availability)(capabilities)
            };

            assert_eq!(
                availability(CommandId::Help),
                if state == AppState::ShuttingDown {
                    CommandAvailability::Hidden
                } else {
                    CommandAvailability::Enabled
                }
            );
            assert_eq!(
                availability(CommandId::Devices),
                if state == AppState::Browsing {
                    CommandAvailability::Enabled
                } else {
                    CommandAvailability::Hidden
                }
            );
            assert_eq!(
                availability(CommandId::Connect),
                if state == AppState::Browsing {
                    CommandAvailability::Enabled
                } else {
                    CommandAvailability::Hidden
                },
                "state: {state:?}"
            );
            assert_eq!(
                availability(CommandId::Send),
                match state {
                    AppState::SessionIdle => CommandAvailability::Enabled,
                    AppState::ClosingSession => {
                        CommandAvailability::Disabled("The session is closing")
                    }
                    state if state.has_session() => {
                        CommandAvailability::Disabled("Transfer already active")
                    }
                    _ => CommandAvailability::Hidden,
                },
                "state: {state:?}"
            );
            assert_eq!(
                availability(CommandId::Disconnect),
                match state {
                    state if state.can_disconnect() => CommandAvailability::Enabled,
                    AppState::ClosingPairing | AppState::ClosingSession => {
                        CommandAvailability::Disabled("Disconnect already in progress")
                    }
                    _ => CommandAvailability::Hidden,
                },
                "state: {state:?}"
            );
        }
    }

    #[test]
    fn filtering_selection_and_resolution_use_capabilities() {
        let browsing = AppModel::for_test(AppState::Browsing).capabilities();
        assert_eq!(
            visible_commands(browsing, "DEV")
                .iter()
                .map(|command| command.id)
                .collect::<Vec<_>>(),
            [CommandId::Devices]
        );
        assert_eq!(first_visible(browsing, "missing"), None);

        let session = AppModel::for_test(AppState::SessionIdle).capabilities();
        assert_eq!(
            resolve(session, "send", Some(CommandId::Send)),
            Some(UserAction::StartTransfer)
        );
        let busy = AppModel::for_test(AppState::OutboundProposal).capabilities();
        assert_eq!(resolve(busy, "send", Some(CommandId::Send)), None);
        assert_eq!(resolve(session, "help", Some(CommandId::Quit)), None);
        assert_eq!(
            resolve(browsing, "connect", Some(CommandId::Connect)),
            Some(UserAction::OpenDirectAddress)
        );
        assert_eq!(resolve(session, "connect", Some(CommandId::Connect)), None);

        assert_eq!(
            move_selection(browsing, "", Some(CommandId::Quit), true),
            Some(CommandId::Help)
        );
        assert_eq!(
            reconcile_selection(browsing, "devices", Some(CommandId::Quit)),
            Some(CommandId::Devices)
        );
    }
}
