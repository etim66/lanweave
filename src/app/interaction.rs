//! Terminal-independent interaction state and key handling.

use std::fmt;
use std::path::{Path, PathBuf};

use super::action::{
    DirectAddressError, DirectEndpoint, KeyInput, MAX_DIRECT_ADDRESS_CHARS, UserAction,
};
use super::command_palette::{
    CommandId, MAX_COMMAND_QUERY_CHARS, first_visible, move_selection, reconcile_selection, resolve,
};
use super::model::{AppModel, AppState};
use crate::discovery::truncate_utf8;
use crate::pairing::{CODE_DIGITS, PairingCode};
use crate::storage::validate_destination;
use crate::transfer::selection::{FileSelection, MAX_SELECTION_INPUT_BYTES, SelectionIssue};
use crate::update::UpdateCheck;

/// Maximum size in bytes of the destination path input.
pub(crate) const MAX_DESTINATION_INPUT_BYTES: usize = 4_096;
/// Rows moved by PageUp and PageDown in scrollable lists.
const LIST_PAGE_ROWS: usize = 10;

/// Live query and selection for the open command palette.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommandPalette {
    pub(crate) query: String,
    pub(crate) selected: Option<CommandId>,
}

/// Which of a dialog's two decision buttons is focused.
///
/// The focused button is activated with Enter; the arrow keys move between
/// buttons so a non-developer never has to discover Escape.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum DialogFocus {
    #[default]
    Accept,
    Reject,
}

impl DialogFocus {
    /// Moves the focus to the other button.
    fn toggle(&mut self) {
        *self = match self {
            Self::Accept => Self::Reject,
            Self::Reject => Self::Accept,
        };
    }
}

/// Live focus for the inbound pairing decision dialog.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct PairingPromptInput {
    pub(crate) focus: DialogFocus,
}

/// Live direct-address input line and its validation result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DirectAddressInput {
    pub(crate) text: String,
    pub(crate) error: Option<DirectAddressError>,
}

impl DirectAddressInput {
    /// Creates an empty input with no error.
    fn new() -> Self {
        Self {
            text: String::new(),
            error: None,
        }
    }

    /// Applies one edit and re-validates the whole line.
    fn edit(&mut self, apply: impl FnOnce(&mut String)) {
        apply(&mut self.text);
        self.error = DirectEndpoint::parse(&self.text).err();
    }
}

/// Live eight-digit pairing-code entry for the initiator.
///
/// The digits are redacted from `Debug` because the entered code is as
/// sensitive as the code shown on the responder's screen.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct PairingCodeInput {
    pub(crate) digits: String,
}

impl fmt::Debug for PairingCodeInput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PairingCodeInput")
            .field("digits", &"[REDACTED]")
            .finish()
    }
}

impl PairingCodeInput {
    /// Creates an empty input.
    fn new() -> Self {
        Self {
            digits: String::new(),
        }
    }

    /// Appends one digit, bounded to the exact code length.
    fn push(&mut self, character: char) {
        if self.digits.len() < CODE_DIGITS && character.is_ascii_digit() {
            self.digits.push(character);
        }
    }

    /// Removes the last entered digit.
    fn pop(&mut self) {
        self.digits.pop();
    }

    /// Formats the entered digits grouped as `1234 5678`.
    pub(crate) fn grouped(&self) -> String {
        let mut display = String::with_capacity(self.digits.len() + 1);
        for (index, character) in self.digits.chars().enumerate() {
            if index == CODE_DIGITS / 2 {
                display.push(' ');
            }
            display.push(character);
        }
        display
    }
}

/// A full-screen UI surface drawn above the current screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Overlay {
    CommandPalette(CommandPalette),
    DirectAddress(DirectAddressInput),
    PairingPrompt(PairingPromptInput),
    PairingCode(PairingCodeInput),
    FileSelection(FileSelectionInput),
    TransferReview(TransferReviewInput),
    Update(UpdateInput),
    Help,
}

/// Live local file review: pending text, reviewed files, issues, and cursor.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct FileSelectionInput {
    /// Pasted or typed paths awaiting review.
    pub(crate) text: String,
    /// Files already validated and reviewed.
    pub(crate) selection: FileSelection,
    /// Rejections from the last review pass.
    pub(crate) issues: Vec<SelectionIssue>,
    /// Highlighted entry for removal.
    pub(crate) selected: Option<usize>,
    /// Set while an off-thread review owns the overlay.
    pub(crate) reviewing: bool,
    /// Identifies the in-flight review so stale results are ignored.
    pub(crate) review_id: u64,
}

impl FileSelectionInput {
    /// Returns whether an off-thread review is still running.
    pub(crate) const fn is_reviewing(&self) -> bool {
        self.reviewing
    }
}

/// Live inbound transfer review: the chosen destination and its validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TransferReviewInput {
    /// Directory typed by the recipient; prefilled with the default.
    pub(crate) destination: String,
    /// Why the current destination cannot be accepted.
    pub(crate) error: Option<&'static str>,
    /// Which decision button is focused.
    pub(crate) focus: DialogFocus,
    /// Highlighted manifest row while the file list scrolls.
    pub(crate) scroll: usize,
}

impl TransferReviewInput {
    /// Creates a review prefilled with the default destination.
    fn new(destination: PathBuf) -> Self {
        Self {
            destination: destination.to_string_lossy().into_owned(),
            error: None,
            focus: DialogFocus::Accept,
            scroll: 0,
        }
    }
}

/// Live self-update dialog: the current phase and the focused button.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UpdateInput {
    pub(crate) phase: UpdatePhase,
    pub(crate) focus: DialogFocus,
}

impl UpdateInput {
    /// Creates the dialog while a release check is running.
    fn checking() -> Self {
        Self {
            phase: UpdatePhase::Checking,
            focus: DialogFocus::Accept,
        }
    }
}

/// What the self-update dialog is currently showing.
///
/// Version strings come from the release tag; the view escapes them before
/// display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum UpdatePhase {
    /// A release check is running off the event loop.
    Checking,
    /// The running version is the newest stable release.
    UpToDate { current: String },
    /// This copy was not installed by the release installer.
    NotManaged,
    /// A newer release waits for the local decision.
    Available { current: String, new: String },
    /// The installer is running.
    Installing { new: String },
    /// The new version is on disk; a restart is required to run it.
    Installed { new: String },
    /// The check or install failed.
    Failed { message: String },
}

/// Scroll position of a file list.
///
/// While `follow` is set, the view anchors the window to the active transfer's
/// current file instead of `cursor`; manual scrolling clears it so the user
/// can inspect earlier entries, and End returns to the live view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FileListScroll {
    cursor: usize,
    follow: bool,
}

impl FileListScroll {
    /// A scroll position that tracks the active transfer's current file.
    const FOLLOWING: Self = Self {
        cursor: 0,
        follow: true,
    };

    /// Returns the highlighted row.
    pub(crate) const fn cursor(self) -> usize {
        self.cursor
    }

    /// Returns whether the cursor tracks the active transfer's current file.
    pub(crate) const fn follow(self) -> bool {
        self.follow
    }
}

impl Default for FileListScroll {
    fn default() -> Self {
        Self::FOLLOWING
    }
}

/// Terminal-independent UI state that the view renders.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct UiState {
    overlay: Option<Overlay>,
    /// Newer release found by a check, shown as a home-screen notice.
    update_available: Option<String>,
    device_selection: Option<super::action::DeviceId>,
    /// Scroll position of the active-transfer file list.
    transfer_scroll: FileListScroll,
    /// Highlighted row of the finished-transfer summary.
    summary_scroll: usize,
    /// The last state seen by [`reconcile`], used to reset per-transfer scroll.
    last_state: Option<AppState>,
}

impl UiState {
    /// Returns the active overlay, if any.
    pub(crate) const fn overlay(&self) -> Option<&Overlay> {
        self.overlay.as_ref()
    }

    /// Builds UI state around one overlay for rendering tests.
    #[cfg(test)]
    pub(crate) const fn for_test(overlay: Overlay) -> Self {
        Self {
            overlay: Some(overlay),
            update_available: None,
            device_selection: None,
            transfer_scroll: FileListScroll::FOLLOWING,
            summary_scroll: 0,
            last_state: None,
        }
    }

    /// Builds UI state carrying an offered update and no overlay.
    #[cfg(test)]
    pub(crate) fn for_test_update(version: &str) -> Self {
        Self {
            update_available: Some(version.to_owned()),
            ..Self::default()
        }
    }

    /// Returns the newer release offered by the last successful check.
    pub(crate) fn update_available(&self) -> Option<&str> {
        self.update_available.as_deref()
    }

    /// Returns the currently selected device in the browsing list.
    pub(crate) const fn device_selection(&self) -> Option<super::action::DeviceId> {
        self.device_selection
    }

    /// Returns the scroll position of the active-transfer file list.
    pub(crate) const fn transfer_scroll(&self) -> FileListScroll {
        self.transfer_scroll
    }

    /// Returns the highlighted row of the finished-transfer summary.
    pub(crate) const fn summary_scroll(&self) -> usize {
        self.summary_scroll
    }

    /// Closes any open overlay.
    pub(super) fn clear(&mut self) {
        self.overlay = None;
    }
}

/// Interprets one key against the current UI state.
///
/// Returns a user action when the key completes one, and otherwise updates
/// `ui` in place. Typing in the palette is capped at
/// [`MAX_COMMAND_QUERY_CHARS`].
pub(crate) fn apply_key_input(
    model: &AppModel,
    ui: &mut UiState,
    input: KeyInput,
) -> Option<UserAction> {
    match ui.overlay.take() {
        Some(Overlay::CommandPalette(mut palette)) => {
            match input {
                KeyInput::Character(character) => {
                    if palette.query.chars().count() < MAX_COMMAND_QUERY_CHARS {
                        palette.query.push(character);
                    }
                    palette.selected = first_visible(model.capabilities(), &palette.query);
                }
                KeyInput::Backspace => {
                    palette.query.pop();
                    palette.selected = first_visible(model.capabilities(), &palette.query);
                }
                KeyInput::Up => {
                    palette.selected = move_selection(
                        model.capabilities(),
                        &palette.query,
                        palette.selected,
                        false,
                    );
                }
                KeyInput::Down => {
                    palette.selected = move_selection(
                        model.capabilities(),
                        &palette.query,
                        palette.selected,
                        true,
                    );
                }
                KeyInput::Enter => {
                    if let Some(action) =
                        resolve(model.capabilities(), &palette.query, palette.selected)
                    {
                        return Some(action);
                    }
                }
                KeyInput::Escape => return None,
                KeyInput::Left
                | KeyInput::Right
                | KeyInput::PageUp
                | KeyInput::PageDown
                | KeyInput::Home
                | KeyInput::End => {}
            }
            ui.overlay = Some(Overlay::CommandPalette(palette));
            None
        }
        Some(Overlay::DirectAddress(mut address)) => {
            match input {
                KeyInput::Character(character) => {
                    if address.text.chars().count() < MAX_DIRECT_ADDRESS_CHARS {
                        address.edit(|text| text.push(character));
                    }
                }
                KeyInput::Backspace => address.edit(|text| {
                    text.pop();
                }),
                KeyInput::Enter => match DirectEndpoint::parse(&address.text) {
                    Ok(endpoint) => return Some(UserAction::ConnectDirect(endpoint)),
                    Err(error) => address.error = Some(error),
                },
                KeyInput::Escape => return None,
                _ => {}
            }
            ui.overlay = Some(Overlay::DirectAddress(address));
            None
        }
        Some(Overlay::PairingPrompt(mut prompt)) => {
            match input {
                KeyInput::Left | KeyInput::Right | KeyInput::Up | KeyInput::Down => {
                    prompt.focus.toggle();
                }
                KeyInput::Enter => {
                    return Some(match prompt.focus {
                        DialogFocus::Accept => UserAction::AcceptPairing,
                        DialogFocus::Reject => UserAction::RejectPairing,
                    });
                }
                KeyInput::Character(character) if character.eq_ignore_ascii_case(&'q') => {
                    return Some(UserAction::Quit);
                }
                // Escape stays a shortcut for the focused reject choice.
                KeyInput::Escape => return Some(UserAction::RejectPairing),
                _ => {}
            }
            ui.overlay = Some(Overlay::PairingPrompt(prompt));
            None
        }
        Some(Overlay::PairingCode(mut code_input)) => {
            match input {
                KeyInput::Character(character) if character.is_ascii_digit() => {
                    code_input.push(character);
                }
                KeyInput::Backspace => code_input.pop(),
                KeyInput::Enter => {
                    if let Some(code) = PairingCode::parse(&code_input.digits) {
                        return Some(UserAction::SubmitPairingCode(code));
                    }
                }
                // Cancelling code entry closes the provisional connection.
                KeyInput::Escape => return Some(UserAction::Disconnect),
                _ => {}
            }
            ui.overlay = Some(Overlay::PairingCode(code_input));
            None
        }
        Some(Overlay::FileSelection(mut files)) => {
            // While an off-thread review owns the list, text edits buffer for
            // the next pass and list actions wait; escape still closes.
            if files.reviewing {
                match input {
                    KeyInput::Character(character) => {
                        if files.text.len() < MAX_SELECTION_INPUT_BYTES {
                            files.text.push(character);
                        }
                    }
                    KeyInput::Backspace => {
                        files.text.pop();
                    }
                    KeyInput::Escape => return None,
                    _ => {}
                }
                ui.overlay = Some(Overlay::FileSelection(files));
                return None;
            }
            match input {
                KeyInput::Character(character) => {
                    if files.text.len() < MAX_SELECTION_INPUT_BYTES {
                        files.text.push(character);
                    }
                }
                KeyInput::Backspace => {
                    if files.text.pop().is_none() {
                        remove_selected_file(&mut files);
                    }
                }
                KeyInput::Enter => {
                    if files.text.is_empty() {
                        // An empty input sends the reviewed selection when the
                        // session is authorized and idle.
                        if model.capabilities().can_start_transfer && !files.selection.is_empty() {
                            return Some(UserAction::StartTransfer(std::mem::take(
                                &mut files.selection,
                            )));
                        }
                    } else {
                        files.issues = files.selection.add_text(&files.text);
                        files.text.clear();
                        if files.selected.is_none() && !files.selection.is_empty() {
                            files.selected = Some(0);
                        }
                    }
                }
                KeyInput::Up => move_file_selection(&mut files, false),
                KeyInput::Down => move_file_selection(&mut files, true),
                KeyInput::PageUp | KeyInput::PageDown | KeyInput::Home | KeyInput::End => {
                    move_file_selection_page(&mut files, input);
                }
                KeyInput::Left | KeyInput::Right => {}
                KeyInput::Escape => return None,
            }
            ui.overlay = Some(Overlay::FileSelection(files));
            None
        }
        Some(Overlay::TransferReview(mut review)) => {
            let file_count = model
                .transfer_proposal()
                .map_or(0, |proposal| proposal.files().len());
            match input {
                KeyInput::Character(character) => {
                    if review.destination.len() + character.len_utf8()
                        <= MAX_DESTINATION_INPUT_BYTES
                    {
                        review.destination.push(character);
                        review.error = None;
                    }
                }
                KeyInput::Backspace => {
                    review.destination.pop();
                    review.error = None;
                }
                KeyInput::Left | KeyInput::Right => review.focus.toggle(),
                KeyInput::Up
                | KeyInput::Down
                | KeyInput::PageUp
                | KeyInput::PageDown
                | KeyInput::Home
                | KeyInput::End => scroll_cursor(&mut review.scroll, file_count, input),
                KeyInput::Enter => match review.focus {
                    DialogFocus::Reject => return Some(UserAction::RejectTransfer),
                    DialogFocus::Accept => {
                        match validate_destination(Path::new(&review.destination)) {
                            Ok(()) => {
                                return Some(UserAction::AcceptTransfer(PathBuf::from(
                                    &review.destination,
                                )));
                            }
                            Err(_) => review.error = Some("Enter an existing directory"),
                        }
                    }
                },
                // Escape stays a shortcut for the focused reject choice.
                KeyInput::Escape => return Some(UserAction::RejectTransfer),
            }
            ui.overlay = Some(Overlay::TransferReview(review));
            None
        }
        Some(Overlay::Update(mut update)) => {
            let decision = match (&update.phase, input) {
                (
                    UpdatePhase::Available { .. } | UpdatePhase::Installed { .. },
                    KeyInput::Left | KeyInput::Right | KeyInput::Up | KeyInput::Down,
                ) => {
                    update.focus.toggle();
                    None
                }
                (UpdatePhase::Available { .. }, KeyInput::Enter) => Some(match update.focus {
                    DialogFocus::Accept => UserAction::ApplyUpdate,
                    DialogFocus::Reject => UserAction::DismissUpdate,
                }),
                // The finished update offers a restart now or later.
                (UpdatePhase::Installed { .. }, KeyInput::Enter) => Some(match update.focus {
                    DialogFocus::Accept => UserAction::Quit,
                    DialogFocus::Reject => UserAction::DismissUpdate,
                }),
                (
                    UpdatePhase::UpToDate { .. }
                    | UpdatePhase::NotManaged
                    | UpdatePhase::Failed { .. },
                    KeyInput::Enter | KeyInput::Escape,
                ) => Some(UserAction::DismissUpdate),
                (
                    UpdatePhase::Available { .. } | UpdatePhase::Installed { .. },
                    KeyInput::Escape,
                ) => Some(UserAction::DismissUpdate),
                // A running check or install ignores input so its result is
                // never orphaned.
                _ => None,
            };
            if let Some(action) = decision {
                return Some(action);
            }
            ui.overlay = Some(Overlay::Update(update));
            None
        }
        Some(Overlay::Help) => match input {
            KeyInput::Escape => None,
            KeyInput::Character('/') => {
                open_palette(model, ui);
                None
            }
            KeyInput::Character(character) if character.eq_ignore_ascii_case(&'q') => {
                Some(UserAction::Quit)
            }
            _ => {
                ui.overlay = Some(Overlay::Help);
                None
            }
        },
        None => match input {
            KeyInput::Character('/') => {
                open_palette(model, ui);
                None
            }
            KeyInput::Character(character) if character.eq_ignore_ascii_case(&'q') => {
                Some(UserAction::Quit)
            }
            KeyInput::Enter | KeyInput::Escape if model.state() == AppState::TransferComplete => {
                Some(UserAction::DismissSummary)
            }
            // Waiting screens show one highlighted action; Enter activates it
            // and Escape stays a shortcut.
            KeyInput::Enter | KeyInput::Escape
                if matches!(model.state(), AppState::OutboundProposal)
                    || model.state().is_transfer_active() =>
            {
                Some(UserAction::CancelTransfer)
            }
            KeyInput::Enter | KeyInput::Escape
                if matches!(
                    model.state(),
                    AppState::PairingOutbound
                        | AppState::PairingConfirming
                        | AppState::PairingInboundAccepted
                ) =>
            {
                Some(UserAction::Disconnect)
            }
            // The pairing prompt is a dialog now, but the direct keys stay for
            // terminals that deliver no dialog overlay.
            KeyInput::Enter if model.state() == AppState::PairingInbound => {
                Some(UserAction::AcceptPairing)
            }
            KeyInput::Escape if model.state() == AppState::PairingInbound => {
                Some(UserAction::RejectPairing)
            }
            // Escape leaves the device list for the home screen.
            KeyInput::Escape if model.capabilities().can_show_home => Some(UserAction::GoHome),
            // The active transfer list follows its current file until a manual
            // scroll pauses it; End returns to the live view.
            KeyInput::Up
            | KeyInput::Down
            | KeyInput::PageUp
            | KeyInput::PageDown
            | KeyInput::Home
            | KeyInput::End
                if model.state().is_transfer_active() =>
            {
                scroll_transfer(model, ui, input);
                None
            }
            KeyInput::Up
            | KeyInput::Down
            | KeyInput::PageUp
            | KeyInput::PageDown
            | KeyInput::Home
            | KeyInput::End
                if model.state() == AppState::TransferComplete =>
            {
                let len = model.summary().map_or(0, |summary| summary.files.len());
                scroll_cursor(&mut ui.summary_scroll, len, input);
                None
            }
            KeyInput::PageUp | KeyInput::PageDown | KeyInput::Home | KeyInput::End
                if model.capabilities().can_show_devices =>
            {
                move_device_page(model, ui, input);
                None
            }
            KeyInput::Up | KeyInput::Down if model.capabilities().can_show_devices => {
                move_device_selection(model, ui, input == KeyInput::Down);
                None
            }
            KeyInput::Enter if model.capabilities().can_show_devices => {
                resolve_selected_device(model, ui)
            }
            _ => None,
        },
    }
}

/// Appends pasted text to the open path input, bounded by the input limit.
///
/// Pasting into the send review buffers the paths for the next review pass and
/// opens the review first when no overlay is open and the state can host it.
/// The runtime reviews the buffered text off the event loop, so a large folder
/// never blocks the terminal. The destination field keeps its plain append
/// behavior.
pub(crate) fn apply_paste(model: &AppModel, ui: &mut UiState, text: &str) {
    match ui.overlay.as_mut() {
        Some(Overlay::FileSelection(files)) => append_pasted(files, text),
        Some(Overlay::TransferReview(review)) => {
            let remaining = MAX_DESTINATION_INPUT_BYTES.saturating_sub(review.destination.len());
            if remaining > 0 {
                review.destination.push_str(&truncate_utf8(text, remaining));
                review.error = None;
            }
        }
        None if model.capabilities().can_review_files && !text.trim().is_empty() => {
            let mut files = FileSelectionInput::default();
            append_pasted(&mut files, text);
            if !files.text.is_empty() {
                ui.overlay = Some(Overlay::FileSelection(files));
            }
        }
        _ => {}
    }
}

/// Appends pasted text to the pending review input without touching the disk.
fn append_pasted(files: &mut FileSelectionInput, text: &str) {
    let remaining = MAX_SELECTION_INPUT_BYTES.saturating_sub(files.text.len());
    if remaining > 0 {
        files.text.push_str(&truncate_utf8(text, remaining));
    }
}

/// Takes pending review text and its base selection for off-thread review.
///
/// Returns `None` when no review list is open or a review is already running.
/// The caller assigns `id` and applies the finished review later.
pub(crate) fn take_review_work(ui: &mut UiState, id: u64) -> Option<(FileSelection, String)> {
    let Some(Overlay::FileSelection(files)) = ui.overlay.as_mut() else {
        return None;
    };
    if files.reviewing || files.text.is_empty() {
        return None;
    }
    files.reviewing = true;
    files.review_id = id;
    Some((files.selection.clone(), std::mem::take(&mut files.text)))
}

/// Applies a finished off-thread review to the open overlay.
///
/// The result is dropped when the overlay closed, was replaced, or belongs to
/// an older review.
pub(crate) fn apply_review(
    ui: &mut UiState,
    id: u64,
    selection: FileSelection,
    issues: Vec<SelectionIssue>,
) {
    let Some(Overlay::FileSelection(files)) = ui.overlay.as_mut() else {
        return;
    };
    if !files.reviewing || files.review_id != id {
        return;
    }
    files.selection = selection;
    files.issues = issues;
    files.reviewing = false;
    files.selected = if files.selection.is_empty() {
        None
    } else {
        Some(files.selected.unwrap_or(0).min(files.selection.len() - 1))
    };
}

/// Applies a finished release check to the home notice and the open dialog.
///
/// A newer release is remembered even without the dialog so the home screen
/// can offer `/update`. A failed check changes nothing: an earlier offer
/// survives it and a first failure shows no notice. The dialog result is
/// dropped when the dialog closed or moved past its checking phase, which can
/// happen after a required prompt takes over the screen.
pub(crate) fn apply_update_check(ui: &mut UiState, check: UpdateCheck) {
    match &check {
        UpdateCheck::Available { new, .. } => ui.update_available = Some(new.clone()),
        UpdateCheck::UpToDate { .. } | UpdateCheck::NotManaged => ui.update_available = None,
        UpdateCheck::Failed(_) => {}
    }

    let Some(Overlay::Update(update)) = ui.overlay.as_mut() else {
        return;
    };
    if update.phase != UpdatePhase::Checking {
        return;
    }
    update.phase = match check {
        UpdateCheck::UpToDate { current } => UpdatePhase::UpToDate { current },
        UpdateCheck::Available { current, new } => UpdatePhase::Available { current, new },
        UpdateCheck::NotManaged => UpdatePhase::NotManaged,
        UpdateCheck::Failed(message) => UpdatePhase::Failed { message },
    };
    update.focus = DialogFocus::Accept;
}

/// Applies a finished update to the home notice and the open dialog.
///
/// A successful install clears the offer; a failure leaves it in place. The
/// dialog result is dropped when the dialog closed or is no longer installing.
pub(crate) fn apply_update_result(ui: &mut UiState, result: Result<String, String>) {
    if result.is_ok() {
        ui.update_available = None;
    }
    let Some(Overlay::Update(update)) = ui.overlay.as_mut() else {
        return;
    };
    if !matches!(update.phase, UpdatePhase::Installing { .. }) {
        return;
    }
    update.phase = match result {
        Ok(new) => UpdatePhase::Installed { new },
        Err(message) => UpdatePhase::Failed { message },
    };
    update.focus = DialogFocus::Accept;
}

/// Applies an already resolved action to the UI state.
///
/// Returns `true` when the action changed the UI and `false` when it must be
/// forwarded to the application model instead.
pub(crate) fn apply_user_action(model: &AppModel, ui: &mut UiState, action: UserAction) -> bool {
    match action {
        UserAction::ShowHelp => {
            ui.overlay = Some(Overlay::Help);
            true
        }
        UserAction::OpenDirectAddress => {
            ui.overlay = Some(Overlay::DirectAddress(DirectAddressInput::new()));
            true
        }
        UserAction::OpenFileSelection => {
            // A selection withdrawn by a simultaneous proposal or a rejection
            // is restored so it can be sent again explicitly.
            let selection = model.deferred_selection().cloned().unwrap_or_default();
            ui.overlay = Some(Overlay::FileSelection(FileSelectionInput {
                selection,
                ..FileSelectionInput::default()
            }));
            true
        }
        UserAction::GoHome => {
            // The list highlight is not carried over to the next visit.
            ui.device_selection = None;
            false
        }
        UserAction::CheckForUpdate => {
            // The model emits the check effect; the UI opens the dialog.
            ui.overlay = Some(Overlay::Update(UpdateInput::checking()));
            false
        }
        UserAction::ApplyUpdate => {
            if let Some(Overlay::Update(update)) = ui.overlay.as_mut()
                && let UpdatePhase::Available { new, .. } = &update.phase
            {
                update.phase = UpdatePhase::Installing { new: new.clone() };
            }
            false
        }
        UserAction::DismissUpdate => {
            if matches!(ui.overlay, Some(Overlay::Update(_))) {
                ui.overlay = None;
                true
            } else {
                false
            }
        }
        _ => false,
    }
}

/// Re-validates the UI state after the application model changed.
///
/// The palette selection is kept on the first still-visible command, and a
/// device selection is cleared when its device disappeared from discovery.
/// The initiator's code input opens when the request is accepted and closes
/// as soon as the flow leaves that state; the inbound transfer review opens on
/// every proposal and closes as soon as it is decided or withdrawn.
pub(crate) fn reconcile(model: &AppModel, ui: &mut UiState) {
    reset_scroll_for_new_state(model, ui);

    if let Some(Overlay::CommandPalette(palette)) = ui.overlay.as_mut() {
        palette.selected =
            reconcile_selection(model.capabilities(), &palette.query, palette.selected);
    }

    // The inbound pairing prompt is a required decision dialog.
    if model.state() == AppState::PairingInbound {
        if !matches!(ui.overlay, Some(Overlay::PairingPrompt(_))) {
            ui.overlay = Some(Overlay::PairingPrompt(PairingPromptInput::default()));
        }
    } else if matches!(ui.overlay, Some(Overlay::PairingPrompt(_))) {
        ui.overlay = None;
    }

    if model.state() == AppState::PairingOutboundAccepted {
        if !matches!(ui.overlay, Some(Overlay::PairingCode(_))) {
            ui.overlay = Some(Overlay::PairingCode(PairingCodeInput::new()));
        }
    } else if matches!(ui.overlay, Some(Overlay::PairingCode(_))) {
        ui.overlay = None;
    }

    // The inbound review is a required prompt: it replaces any other overlay
    // and keeps the recipient's destination edits until the proposal ends.
    if model.state() == AppState::InboundProposal {
        if !matches!(ui.overlay, Some(Overlay::TransferReview(_))) {
            ui.overlay = Some(Overlay::TransferReview(TransferReviewInput::new(
                model.default_destination().to_path_buf(),
            )));
        }
    } else if matches!(ui.overlay, Some(Overlay::TransferReview(_))) {
        ui.overlay = None;
    }

    // The review list is local, but it must never hide a required prompt or a
    // running transfer.
    if matches!(ui.overlay, Some(Overlay::FileSelection(_)))
        && !model.capabilities().can_review_files
    {
        ui.overlay = None;
    }

    if let Some(selected) = ui.device_selection {
        let present = model
            .candidates()
            .iter()
            .any(|candidate| candidate.id() == selected);
        if !present {
            ui.device_selection = None;
        }
    }
}

/// Resets per-transfer scroll positions when the state changes.
///
/// The active-transfer list always opens following the current file and the
/// summary always opens at its first row, so a paused or scrolled view never
/// leaks into the next transfer.
fn reset_scroll_for_new_state(model: &AppModel, ui: &mut UiState) {
    let state = model.state();
    if ui.last_state == Some(state) {
        return;
    }
    if state.is_transfer_active() {
        ui.transfer_scroll = FileListScroll::FOLLOWING;
    }
    if state == AppState::TransferComplete {
        ui.summary_scroll = 0;
    }
    ui.last_state = Some(state);
}

/// Moves the browsing selection by one sorted candidate, wrapping at the ends.
fn move_device_selection(model: &AppModel, ui: &mut UiState, forward: bool) {
    let candidates = model.sorted_candidates();
    if candidates.is_empty() {
        ui.device_selection = None;
        return;
    }

    let current = ui.device_selection.and_then(|selected| {
        candidates
            .iter()
            .position(|candidate| candidate.id() == selected)
    });
    let next = match (current, forward) {
        (Some(index), true) => (index + 1) % candidates.len(),
        (Some(0), false) | (None, false) => candidates.len() - 1,
        (None, true) => 0,
        (Some(index), false) => index - 1,
    };
    ui.device_selection = Some(candidates[next].id());
}

/// Moves the browsing selection by one page or to an end of the list.
fn move_device_page(model: &AppModel, ui: &mut UiState, input: KeyInput) {
    let candidates = model.sorted_candidates();
    if candidates.is_empty() {
        ui.device_selection = None;
        return;
    }

    let current = ui
        .device_selection
        .and_then(|selected| {
            candidates
                .iter()
                .position(|candidate| candidate.id() == selected)
        })
        .unwrap_or(0);
    let mut cursor = current;
    scroll_cursor(&mut cursor, candidates.len(), input);
    ui.device_selection = Some(candidates[cursor].id());
}

/// Resolves the selected device against the live candidate store.
///
/// A stale selection (the device disappeared) resolves to nothing and is
/// cleared, so a removed device can never be connected through the UI.
fn resolve_selected_device(model: &AppModel, ui: &mut UiState) -> Option<UserAction> {
    let selected = ui.device_selection?;
    if model
        .candidates()
        .iter()
        .any(|candidate| candidate.id() == selected)
    {
        Some(UserAction::SelectDevice(selected))
    } else {
        ui.device_selection = None;
        None
    }
}

/// Moves the review selection by one entry, wrapping at the ends.
fn move_file_selection(files: &mut FileSelectionInput, forward: bool) {
    let count = files.selection.len();
    if count == 0 {
        files.selected = None;
        return;
    }

    let next = match (files.selected, forward) {
        (Some(index), true) => (index + 1) % count,
        (Some(0), false) | (None, false) => count - 1,
        (None, true) => 0,
        (Some(index), false) => index - 1,
    };
    files.selected = Some(next);
}

/// Removes the highlighted review entry and keeps the cursor valid.
fn remove_selected_file(files: &mut FileSelectionInput) {
    let Some(index) = files.selected else {
        return;
    };
    if files.selection.remove(index) {
        let remaining = files.selection.len();
        files.selected = (remaining > 0).then(|| index.min(remaining - 1));
    }
}

/// Moves the review selection by one page or to an end of the list.
fn move_file_selection_page(files: &mut FileSelectionInput, input: KeyInput) {
    let len = files.selection.len();
    if len == 0 {
        files.selected = None;
        return;
    }
    let mut cursor = files.selected.unwrap_or(0);
    scroll_cursor(&mut cursor, len, input);
    files.selected = Some(cursor);
}

/// Scrolls the active-transfer list, pausing the live follow on manual input.
///
/// While following, the cursor starts at the current file so the first manual
/// scroll moves relative to what is on screen instead of jumping to the top.
fn scroll_transfer(model: &AppModel, ui: &mut UiState, input: KeyInput) {
    let len = tracked_file_count(model);
    let mut cursor = if ui.transfer_scroll.follow {
        model
            .transfer_progress()
            .map_or(ui.transfer_scroll.cursor, |progress| {
                usize::from(progress.index)
            })
    } else {
        ui.transfer_scroll.cursor
    };
    // End returns to the live view; every other key inspects the list.
    ui.transfer_scroll.follow = input == KeyInput::End;
    scroll_cursor(&mut cursor, len, input);
    ui.transfer_scroll.cursor = cursor;
}

/// Returns the number of manifest rows the active transfer shows.
fn tracked_file_count(model: &AppModel) -> usize {
    if model.state() == AppState::TransferringOutbound {
        return model
            .outbound_selection()
            .map_or(0, |selection| selection.len());
    }
    model
        .transfer_proposal()
        .map_or(0, |proposal| proposal.files().len())
}

/// Moves `cursor` by one row, one page, or to an edge of a `len`-row list.
fn scroll_cursor(cursor: &mut usize, len: usize, input: KeyInput) {
    match input {
        KeyInput::Up => move_list_cursor(cursor, len, -1),
        KeyInput::Down => move_list_cursor(cursor, len, 1),
        KeyInput::PageUp => move_list_cursor(cursor, len, -(LIST_PAGE_ROWS as isize)),
        KeyInput::PageDown => move_list_cursor(cursor, len, LIST_PAGE_ROWS as isize),
        KeyInput::Home => move_list_cursor_edge(cursor, len, false),
        KeyInput::End => move_list_cursor_edge(cursor, len, true),
        _ => {}
    }
}

/// Moves a list cursor by `delta` rows, bounded to the list.
fn move_list_cursor(cursor: &mut usize, len: usize, delta: isize) {
    if len == 0 {
        *cursor = 0;
        return;
    }
    let last = len - 1;
    let current = (*cursor).min(last) as isize;
    *cursor = (current + delta).clamp(0, last as isize) as usize;
}

/// Moves a list cursor to its first or last row.
fn move_list_cursor_edge(cursor: &mut usize, len: usize, at_end: bool) {
    *cursor = if len == 0 || !at_end { 0 } else { len - 1 };
}

/// Opens the command palette with an empty query and the first command selected.
fn open_palette(model: &AppModel, ui: &mut UiState) {
    let query = String::new();
    let selected = first_visible(model.capabilities(), &query);
    ui.overlay = Some(Overlay::CommandPalette(CommandPalette { query, selected }));
}

#[cfg(test)]
mod tests {
    use tokio::time::Instant;

    use super::{
        DialogFocus, FileSelectionInput, MAX_DESTINATION_INPUT_BYTES, Overlay, UiState,
        UpdatePhase, apply_key_input, apply_paste, apply_review, apply_update_check,
        apply_update_result, apply_user_action, reconcile, take_review_work,
    };
    use crate::app::action::{DirectAddressError, DirectEndpoint, KeyInput, UserAction};
    use crate::app::event::AppEvent;
    use crate::app::failure::FailureKind;
    use crate::app::model::{
        AppModel, AppState, TransferDirection, TransferProgress, TransferProposal, TransferSummary,
    };
    use crate::app::reducer::update;
    use crate::discovery::{DiscoveredService, DiscoveryEvent};
    use crate::protocol::{FileEntry, TransferRequest};
    use crate::transfer::selection::{FileSelection, MAX_SELECTION_INPUT_BYTES};
    use crate::update::UpdateCheck;

    /// Completes one off-thread review synchronously for tests.
    fn review_pasted(ui: &mut UiState, id: u64) -> bool {
        let Some((mut selection, text)) = take_review_work(ui, id) else {
            return false;
        };
        let issues = selection.add_text(&text);
        apply_review(ui, id, selection, issues);
        true
    }

    /// A bounded inbound manifest for interaction tests.
    fn proposal() -> TransferProposal {
        TransferProposal::new(
            &TransferRequest::new(vec![FileEntry::new("report.txt".to_owned(), 64)]).unwrap(),
            Some("peer".to_owned()),
        )
    }

    fn browsing_with(names: &[&str]) -> AppModel {
        let mut model = AppModel::for_test(AppState::Browsing);
        for name in names {
            model.apply_discovery(DiscoveryEvent::Resolved(DiscoveredService::for_test(
                name,
                Instant::now(),
            )));
        }
        model
    }

    #[test]
    fn palette_query_is_bounded_and_q_is_contextual() {
        let model = AppModel::for_test(AppState::Browsing);
        let mut ui = UiState::default();
        assert_eq!(
            apply_key_input(&model, &mut ui, KeyInput::Character('/')),
            None
        );
        assert_eq!(
            apply_key_input(&model, &mut ui, KeyInput::Character('q')),
            None
        );

        for _ in 0..super::MAX_COMMAND_QUERY_CHARS + 10 {
            apply_key_input(&model, &mut ui, KeyInput::Character('x'));
        }

        let Some(Overlay::CommandPalette(palette)) = ui.overlay() else {
            panic!("palette should remain open");
        };
        assert_eq!(
            palette.query.chars().count(),
            super::MAX_COMMAND_QUERY_CHARS
        );

        apply_key_input(&model, &mut ui, KeyInput::Escape);
        assert_eq!(
            apply_key_input(&model, &mut ui, KeyInput::Character('q')),
            Some(UserAction::Quit)
        );
    }

    #[test]
    fn palette_resolves_actions_and_reconciles_state_changes() {
        let session = AppModel::for_test(AppState::SessionIdle);
        let mut ui = UiState::default();
        apply_key_input(&session, &mut ui, KeyInput::Character('/'));
        for character in "send".chars() {
            apply_key_input(&session, &mut ui, KeyInput::Character(character));
        }
        assert_eq!(
            apply_key_input(&session, &mut ui, KeyInput::Enter),
            Some(UserAction::OpenFileSelection)
        );
        assert_eq!(ui.overlay(), None);

        apply_key_input(&session, &mut ui, KeyInput::Character('/'));
        for character in "send".chars() {
            apply_key_input(&session, &mut ui, KeyInput::Character(character));
        }
        let busy = AppModel::for_test(AppState::OutboundProposal);
        reconcile(&busy, &mut ui);
        assert_eq!(apply_key_input(&busy, &mut ui, KeyInput::Enter), None);
        assert!(matches!(ui.overlay(), Some(Overlay::CommandPalette(_))));
    }

    #[test]
    fn help_is_ui_only_while_devices_reaches_the_model() {
        let model = AppModel::for_test(AppState::Browsing);
        let mut ui = UiState::default();
        assert!(apply_user_action(&model, &mut ui, UserAction::ShowHelp));
        assert_eq!(ui.overlay(), Some(&Overlay::Help));
        // `/devices` is an application action now: the model decides whether
        // the device list opens, so the UI must not swallow it.
        assert!(!apply_user_action(&model, &mut ui, UserAction::ShowDevices));
        assert!(apply_user_action(
            &model,
            &mut ui,
            UserAction::OpenFileSelection
        ));
        assert!(matches!(ui.overlay(), Some(Overlay::FileSelection(_))));
        assert!(!apply_user_action(&model, &mut ui, UserAction::Quit));
    }

    #[test]
    fn paste_is_bounded_and_only_reaches_the_review_input() {
        let model = AppModel::for_test(AppState::Browsing);
        let mut ui = UiState::default();

        // Pasting in a reviewable state opens the send review and buffers the
        // paths for the next off-thread pass.
        apply_paste(&model, &mut ui, "ignored");
        let Some(Overlay::FileSelection(files)) = ui.overlay() else {
            panic!("the review overlay should open");
        };
        assert_eq!(files.text, "ignored");
        assert!(!files.is_reviewing());

        // A state that cannot host the review ignores paste entirely.
        let mut pairing_ui = UiState::default();
        let pairing = AppModel::for_test(AppState::PairingInbound);
        apply_paste(&pairing, &mut pairing_ui, "ignored");
        assert_eq!(pairing_ui.overlay(), None);

        // One paste is truncated to the input limit before it is reviewed.
        let mut bounded = UiState::default();
        apply_paste(
            &model,
            &mut bounded,
            &"x".repeat(MAX_SELECTION_INPUT_BYTES + 10),
        );
        let Some(Overlay::FileSelection(files)) = bounded.overlay() else {
            panic!("the review overlay should open");
        };
        assert_eq!(files.text.len(), MAX_SELECTION_INPUT_BYTES);

        // The runtime takes the buffer, reviews it off-thread, and applies it.
        assert!(review_pasted(&mut bounded, 7));
        let Some(Overlay::FileSelection(files)) = bounded.overlay() else {
            panic!("the review overlay should remain open");
        };
        assert!(!files.is_reviewing());
        assert_eq!(files.text.len(), 0);
        assert_eq!(files.issues.len(), 1);
        assert!(files.issues[0].path().starts_with("xxx"));
    }

    #[test]
    fn a_reviewing_overlay_buffers_text_and_ignores_stale_results() {
        let session = AppModel::for_test(AppState::SessionIdle);
        let mut ui = UiState::default();
        apply_paste(&session, &mut ui, "first.txt");
        assert!(take_review_work(&mut ui, 3).is_some());

        // List actions wait while the review runs; typed text is buffered.
        assert_eq!(apply_key_input(&session, &mut ui, KeyInput::Enter), None);
        assert_eq!(
            apply_key_input(&session, &mut ui, KeyInput::Backspace),
            None
        );
        apply_key_input(&session, &mut ui, KeyInput::Character('x'));
        let Some(Overlay::FileSelection(files)) = ui.overlay() else {
            panic!("the review overlay should remain open");
        };
        assert!(files.is_reviewing());
        assert_eq!(files.text, "x");

        // A result for a closed overlay is dropped.
        assert_eq!(apply_key_input(&session, &mut ui, KeyInput::Escape), None);
        assert_eq!(ui.overlay(), None);
        apply_review(
            &mut ui,
            3,
            FileSelection::for_test(&[("first.txt", 1)]),
            Vec::new(),
        );
        assert_eq!(ui.overlay(), None);

        // A stale id cannot overwrite the review that is still running.
        apply_paste(&session, &mut ui, "second.txt");
        assert!(take_review_work(&mut ui, 4).is_some());
        apply_review(&mut ui, 3, FileSelection::default(), Vec::new());
        let Some(Overlay::FileSelection(files)) = ui.overlay() else {
            panic!("the review overlay should remain open");
        };
        assert!(files.is_reviewing());
        assert!(files.selection.is_empty());

        apply_review(
            &mut ui,
            4,
            FileSelection::for_test(&[("second.txt", 2)]),
            Vec::new(),
        );
        let Some(Overlay::FileSelection(files)) = ui.overlay() else {
            panic!("the review overlay should remain open");
        };
        assert!(!files.is_reviewing());
        assert_eq!(files.selection.len(), 1);
        assert_eq!(files.selected, Some(0));
    }

    #[test]
    fn file_review_adds_removes_and_gates_send() {
        let root = std::env::temp_dir().join(format!("lanweave-review-{:016x}", fastrand::u64(..)));
        std::fs::create_dir_all(&root).unwrap();
        let first = root.join("first.txt");
        let second = root.join("second.txt");
        std::fs::write(&first, b"one").unwrap();
        std::fs::write(&second, b"two").unwrap();

        let mut ui = UiState::default();
        assert!(apply_user_action(
            &AppModel::for_test(AppState::Browsing),
            &mut ui,
            UserAction::OpenFileSelection
        ));
        // An empty review cannot send, even in an idle session.
        let session = AppModel::for_test(AppState::SessionIdle);
        assert_eq!(apply_key_input(&session, &mut ui, KeyInput::Enter), None);

        apply_paste(
            &session,
            &mut ui,
            &format!("{}\n{}\n", first.display(), second.display()),
        );
        assert!(review_pasted(&mut ui, 1));

        // Pasted paths are reviewed off-thread; browsing cannot send them yet.
        let browsing = AppModel::for_test(AppState::Browsing);
        let Some(Overlay::FileSelection(files)) = ui.overlay() else {
            panic!("the review overlay should remain open");
        };
        assert_eq!(files.selection.len(), 2);
        assert_eq!(apply_key_input(&browsing, &mut ui, KeyInput::Enter), None);

        // Down and backspace remove the highlighted entry.
        assert_eq!(apply_key_input(&browsing, &mut ui, KeyInput::Down), None);
        apply_key_input(&browsing, &mut ui, KeyInput::Backspace);
        let Some(Overlay::FileSelection(files)) = ui.overlay() else {
            panic!("the review overlay should remain open");
        };
        assert_eq!(files.selection.len(), 1);

        // An authorized idle session sends the remaining reviewed file.
        assert!(matches!(
            apply_key_input(&session, &mut ui, KeyInput::Enter),
            Some(UserAction::StartTransfer(_))
        ));
        assert_eq!(ui.overlay(), None);

        // Escape closes the review without sending.
        assert!(apply_user_action(
            &session,
            &mut ui,
            UserAction::OpenFileSelection
        ));
        assert_eq!(apply_key_input(&session, &mut ui, KeyInput::Escape), None);
        assert_eq!(ui.overlay(), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn required_prompts_replace_the_review_overlay() {
        let mut ui = UiState::default();
        assert!(apply_user_action(
            &AppModel::for_test(AppState::Browsing),
            &mut ui,
            UserAction::OpenFileSelection
        ));
        assert!(matches!(ui.overlay(), Some(Overlay::FileSelection(_))));

        // The required pairing decision replaces the local review list.
        let pairing = AppModel::for_test(AppState::PairingInbound);
        reconcile(&pairing, &mut ui);
        let Some(Overlay::PairingPrompt(prompt)) = ui.overlay() else {
            panic!("the pairing dialog must open");
        };
        assert_eq!(prompt.focus, DialogFocus::Accept);

        // Leaving the prompt state closes the dialog again.
        reconcile(&AppModel::for_test(AppState::Home), &mut ui);
        assert_eq!(ui.overlay(), None);
    }

    #[test]
    fn device_list_navigation_selects_wraps_and_connects() {
        let model = browsing_with(&["zeta", "alpha"]);
        let mut ui = UiState::default();

        assert_eq!(apply_key_input(&model, &mut ui, KeyInput::Down), None);
        let alpha = model.sorted_candidates()[0].id();
        assert_eq!(ui.device_selection(), Some(alpha));

        assert_eq!(apply_key_input(&model, &mut ui, KeyInput::Down), None);
        let zeta = model.sorted_candidates()[1].id();
        assert_eq!(ui.device_selection(), Some(zeta));

        // Navigation wraps at both ends.
        apply_key_input(&model, &mut ui, KeyInput::Down);
        assert_eq!(ui.device_selection(), Some(alpha));
        apply_key_input(&model, &mut ui, KeyInput::Up);
        assert_eq!(ui.device_selection(), Some(zeta));

        assert_eq!(
            apply_key_input(&model, &mut ui, KeyInput::Enter),
            Some(UserAction::SelectDevice(zeta))
        );

        // An empty list ignores navigation and cannot dispatch.
        let empty = browsing_with(&[]);
        let mut empty_ui = UiState::default();
        assert_eq!(apply_key_input(&empty, &mut empty_ui, KeyInput::Down), None);
        assert_eq!(empty_ui.device_selection(), None);
        assert_eq!(
            apply_key_input(&empty, &mut empty_ui, KeyInput::Enter),
            None
        );
    }

    #[test]
    fn escape_returns_home_from_the_device_list() {
        let model = browsing_with(&["alpha"]);
        let mut ui = UiState::default();
        apply_key_input(&model, &mut ui, KeyInput::Down);
        assert!(ui.device_selection().is_some());

        assert_eq!(
            apply_key_input(&model, &mut ui, KeyInput::Escape),
            Some(UserAction::GoHome)
        );
        // The UI clears the highlight before the reducer changes state.
        assert!(!apply_user_action(&model, &mut ui, UserAction::GoHome));
        assert_eq!(ui.device_selection(), None);

        // An open overlay still consumes Escape first.
        let mut palette_ui = UiState::default();
        apply_key_input(&model, &mut palette_ui, KeyInput::Character('/'));
        assert_eq!(
            apply_key_input(&model, &mut palette_ui, KeyInput::Escape),
            None
        );
        assert_eq!(palette_ui.overlay(), None);

        // Other screens never dispatch the home action from Escape alone.
        for state in [AppState::Home, AppState::SessionIdle] {
            let mut other_ui = UiState::default();
            assert_eq!(
                apply_key_input(&AppModel::for_test(state), &mut other_ui, KeyInput::Escape),
                None,
                "state: {state:?}"
            );
        }
    }

    #[test]
    fn removed_device_clears_selection_and_cannot_connect() {
        let mut model = browsing_with(&["alpha", "beta"]);
        let mut ui = UiState::default();
        apply_key_input(&model, &mut ui, KeyInput::Down);
        assert!(ui.device_selection().is_some());

        model.apply_discovery(DiscoveryEvent::Removed {
            service_instance: "alpha._lanweave._tcp.local.".to_owned(),
        });
        reconcile(&model, &mut ui);

        assert_eq!(ui.device_selection(), None);
        assert_eq!(apply_key_input(&model, &mut ui, KeyInput::Enter), None);

        // A remaining device can still be selected and connected.
        apply_key_input(&model, &mut ui, KeyInput::Down);
        assert_eq!(
            ui.device_selection(),
            Some(model.sorted_candidates()[0].id())
        );
        assert_eq!(
            apply_key_input(&model, &mut ui, KeyInput::Enter),
            Some(UserAction::SelectDevice(model.sorted_candidates()[0].id()))
        );
    }

    #[test]
    fn selection_survives_unrelated_updates_and_list_keys_are_ignored_outside_browsing() {
        let mut model = browsing_with(&["alpha"]);
        let mut ui = UiState::default();
        apply_key_input(&model, &mut ui, KeyInput::Down);
        let alpha = ui.device_selection().unwrap();

        model.apply_discovery(DiscoveryEvent::Resolved(DiscoveredService::for_test(
            "beta",
            Instant::now(),
        )));
        reconcile(&model, &mut ui);
        assert_eq!(ui.device_selection(), Some(alpha));

        for state in [
            AppState::Starting,
            AppState::Home,
            AppState::SessionIdle,
            AppState::Error(FailureKind::Internal),
        ] {
            let mut busy = AppModel::for_test(state);
            busy.apply_discovery(DiscoveryEvent::Resolved(DiscoveredService::for_test(
                "peer",
                Instant::now(),
            )));
            let mut busy_ui = UiState::default();

            assert_eq!(apply_key_input(&busy, &mut busy_ui, KeyInput::Down), None);
            assert_eq!(busy_ui.device_selection(), None, "state: {state:?}");
            assert_eq!(
                apply_key_input(&busy, &mut busy_ui, KeyInput::Enter),
                None,
                "state: {state:?}"
            );
        }
    }

    #[test]
    fn direct_address_input_bounds_validates_and_dispatches() {
        let model = browsing_with(&[]);
        let mut ui = UiState::default();
        assert!(apply_user_action(
            &model,
            &mut ui,
            UserAction::OpenDirectAddress
        ));

        // Input is bounded.
        for _ in 0..super::MAX_DIRECT_ADDRESS_CHARS + 20 {
            apply_key_input(&model, &mut ui, KeyInput::Character('x'));
        }
        let Some(Overlay::DirectAddress(input)) = ui.overlay() else {
            panic!("the input overlay should remain open");
        };
        assert_eq!(input.text.chars().count(), super::MAX_DIRECT_ADDRESS_CHARS);

        // Empty input reports a missing host and never dispatches.
        for _ in 0..super::MAX_DIRECT_ADDRESS_CHARS {
            apply_key_input(&model, &mut ui, KeyInput::Backspace);
        }
        apply_key_input(&model, &mut ui, KeyInput::Enter);
        let Some(Overlay::DirectAddress(input)) = ui.overlay() else {
            panic!("the input overlay should remain open");
        };
        assert_eq!(input.error, Some(DirectAddressError::MissingHost));

        // A host without a port reports a missing port.
        for character in "peer.local".chars() {
            apply_key_input(&model, &mut ui, KeyInput::Character(character));
        }
        let Some(Overlay::DirectAddress(input)) = ui.overlay() else {
            panic!("the input overlay should remain open");
        };
        assert_eq!(input.error, Some(DirectAddressError::MissingPort));

        // An invalid port blocks Enter.
        for character in ":abc".chars() {
            apply_key_input(&model, &mut ui, KeyInput::Character(character));
        }
        assert_eq!(apply_key_input(&model, &mut ui, KeyInput::Enter), None);
        let Some(Overlay::DirectAddress(input)) = ui.overlay() else {
            panic!("the input overlay should remain open");
        };
        assert_eq!(input.error, Some(DirectAddressError::InvalidPort));

        // A valid address dispatches and closes the overlay.
        for _ in 0..3 {
            apply_key_input(&model, &mut ui, KeyInput::Backspace);
        }
        for character in "4242".chars() {
            apply_key_input(&model, &mut ui, KeyInput::Character(character));
        }
        assert_eq!(
            apply_key_input(&model, &mut ui, KeyInput::Enter),
            Some(UserAction::ConnectDirect(
                DirectEndpoint::parse("peer.local:4242").unwrap()
            ))
        );
        assert_eq!(ui.overlay(), None);

        // Escape closes the overlay without dispatching.
        apply_user_action(&model, &mut ui, UserAction::OpenDirectAddress);
        assert_eq!(apply_key_input(&model, &mut ui, KeyInput::Escape), None);
        assert_eq!(ui.overlay(), None);
    }

    #[test]
    fn pairing_prompt_accepts_and_rejects_with_enter_and_escape() {
        let model = AppModel::for_test(AppState::PairingInbound);
        let mut ui = UiState::default();

        assert_eq!(
            apply_key_input(&model, &mut ui, KeyInput::Enter),
            Some(UserAction::AcceptPairing)
        );
        assert_eq!(
            apply_key_input(&model, &mut ui, KeyInput::Escape),
            Some(UserAction::RejectPairing)
        );

        // Arrow keys move the focus to Reject, where Enter rejects.
        reconcile(&model, &mut ui);
        apply_key_input(&model, &mut ui, KeyInput::Right);
        assert_eq!(
            apply_key_input(&model, &mut ui, KeyInput::Enter),
            Some(UserAction::RejectPairing)
        );

        // The waiting screen cancels the request with either key.
        let outbound = AppModel::for_test(AppState::PairingOutbound);
        assert_eq!(
            apply_key_input(&outbound, &mut ui, KeyInput::Enter),
            Some(UserAction::Disconnect)
        );
        assert_eq!(
            apply_key_input(&outbound, &mut ui, KeyInput::Escape),
            Some(UserAction::Disconnect)
        );
    }

    #[test]
    fn waiting_screens_offer_a_visible_cancel_action() {
        // The receiver's code screen and the code check both cancel with Enter
        // or Escape; there is no silent state anymore.
        for state in [
            AppState::PairingInboundAccepted,
            AppState::PairingConfirming,
        ] {
            let model = AppModel::for_test(state);
            let mut ui = UiState::default();
            assert_eq!(
                apply_key_input(&model, &mut ui, KeyInput::Enter),
                Some(UserAction::Disconnect),
                "state: {state:?}"
            );
            assert_eq!(
                apply_key_input(&model, &mut ui, KeyInput::Escape),
                Some(UserAction::Disconnect),
                "state: {state:?}"
            );
        }

        let busy = AppModel::for_test(AppState::OutboundProposal);
        let mut ui = UiState::default();
        assert_eq!(
            apply_key_input(&busy, &mut ui, KeyInput::Enter),
            Some(UserAction::CancelTransfer)
        );
        let active = AppModel::for_test(AppState::TransferringOutbound);
        assert_eq!(
            apply_key_input(&active, &mut ui, KeyInput::Enter),
            Some(UserAction::CancelTransfer)
        );
    }

    #[test]
    fn code_entry_opens_accepts_only_digits_and_dispatches_exactly_eight() {
        let accepted = AppModel::for_test(AppState::PairingOutboundAccepted);
        let mut ui = UiState::default();
        reconcile(&accepted, &mut ui);
        assert!(matches!(ui.overlay(), Some(Overlay::PairingCode(_))));

        // Text keys are ignored; only digits edit the code.
        for character in "12ab34 56".chars() {
            apply_key_input(&accepted, &mut ui, KeyInput::Character(character));
        }
        let Some(Overlay::PairingCode(input)) = ui.overlay() else {
            panic!("the code input should remain open");
        };
        assert_eq!(input.grouped(), "1234 56");
        assert_eq!(apply_key_input(&accepted, &mut ui, KeyInput::Enter), None);

        // The ninth digit is dropped, and Enter dispatches the first eight.
        apply_key_input(&accepted, &mut ui, KeyInput::Character('7'));
        apply_key_input(&accepted, &mut ui, KeyInput::Character('8'));
        apply_key_input(&accepted, &mut ui, KeyInput::Character('9'));
        let Some(Overlay::PairingCode(input)) = ui.overlay() else {
            panic!("the code input should remain open");
        };
        assert_eq!(input.grouped(), "1234 5678");
        assert_eq!(
            apply_key_input(&accepted, &mut ui, KeyInput::Enter),
            Some(UserAction::SubmitPairingCode(
                crate::pairing::PairingCode::parse("12345678").unwrap()
            ))
        );

        // Backspace edits, and Escape cancels the pairing connection.
        let mut ui = UiState::default();
        reconcile(&accepted, &mut ui);
        apply_key_input(&accepted, &mut ui, KeyInput::Character('1'));
        apply_key_input(&accepted, &mut ui, KeyInput::Backspace);
        assert_eq!(
            apply_key_input(&accepted, &mut ui, KeyInput::Escape),
            Some(UserAction::Disconnect)
        );

        // Leaving the code-entry state closes the overlay.
        let confirming = AppModel::for_test(AppState::PairingConfirming);
        reconcile(&confirming, &mut ui);
        assert_eq!(ui.overlay(), None);
    }

    #[test]
    fn escape_cancels_a_pending_or_active_transfer() {
        for state in [
            AppState::OutboundProposal,
            AppState::TransferringOutbound,
            AppState::TransferringInbound,
        ] {
            let model = AppModel::for_test(state);
            let mut ui = UiState::default();

            assert_eq!(
                apply_key_input(&model, &mut ui, KeyInput::Escape),
                Some(UserAction::CancelTransfer),
                "state: {state:?}"
            );
        }

        // Other states never dispatch a cancel from Escape alone.
        let idle = AppModel::for_test(AppState::SessionIdle);
        let mut ui = UiState::default();
        assert_eq!(apply_key_input(&idle, &mut ui, KeyInput::Escape), None);
    }

    #[test]
    fn inbound_review_prefills_and_requires_a_real_destination() {
        let mut model = AppModel::for_test(AppState::SessionIdle);
        update(&mut model, AppEvent::IncomingTransferRequest(proposal()));
        let mut ui = UiState::default();
        reconcile(&model, &mut ui);

        let Some(Overlay::TransferReview(review)) = ui.overlay() else {
            panic!("the inbound review must open with the proposal");
        };
        assert_eq!(review.destination, ".");
        assert_eq!(review.error, None);

        // A missing directory reports an error and never dispatches.
        let missing =
            std::env::temp_dir().join(format!("lanweave-missing-{:016x}", fastrand::u64(..)));
        let Some(Overlay::TransferReview(review)) = ui.overlay.as_mut() else {
            panic!("the inbound review must remain open");
        };
        review.destination = missing.display().to_string();
        assert_eq!(apply_key_input(&model, &mut ui, KeyInput::Enter), None);
        let Some(Overlay::TransferReview(review)) = ui.overlay() else {
            panic!("the inbound review must remain open");
        };
        assert!(review.error.is_some());

        // An existing directory dispatches the chosen destination.
        let root = std::env::temp_dir().join(format!("lanweave-review-{:016x}", fastrand::u64(..)));
        std::fs::create_dir_all(&root).unwrap();
        let Some(Overlay::TransferReview(review)) = ui.overlay.as_mut() else {
            panic!("the inbound review must remain open");
        };
        review.destination = root.display().to_string();
        assert_eq!(
            apply_key_input(&model, &mut ui, KeyInput::Enter),
            Some(UserAction::AcceptTransfer(root.clone()))
        );
        assert_eq!(ui.overlay(), None);

        // Escape rejects the proposal and closes the card.
        reconcile(&model, &mut ui);
        assert_eq!(
            apply_key_input(&model, &mut ui, KeyInput::Escape),
            Some(UserAction::RejectTransfer)
        );
        assert_eq!(ui.overlay(), None);

        // Right focuses Reject, where Enter rejects as well.
        reconcile(&model, &mut ui);
        apply_key_input(&model, &mut ui, KeyInput::Right);
        assert_eq!(
            apply_key_input(&model, &mut ui, KeyInput::Enter),
            Some(UserAction::RejectTransfer)
        );
        assert_eq!(ui.overlay(), None);
        let _ = std::fs::remove_dir_all(&root);

        // The byte bound rejects a multi-byte character that would overflow it.
        reconcile(&model, &mut ui);
        for _ in 0..MAX_DESTINATION_INPUT_BYTES {
            apply_key_input(&model, &mut ui, KeyInput::Character('a'));
        }
        apply_key_input(&model, &mut ui, KeyInput::Character('é'));
        let Some(Overlay::TransferReview(review)) = ui.overlay() else {
            panic!("the inbound review must remain open");
        };
        assert_eq!(review.destination.len(), MAX_DESTINATION_INPUT_BYTES);
    }

    #[test]
    fn send_review_restores_a_deferred_selection() {
        let mut model = AppModel::for_test(AppState::SessionIdle);
        let selection = FileSelection::for_test(&[("report.txt", 7)]);
        update(
            &mut model,
            AppEvent::User(UserAction::StartTransfer(selection.clone())),
        );
        update(&mut model, AppEvent::IncomingTransferRequest(proposal()));
        update(&mut model, AppEvent::ProposalRejected);
        assert_eq!(model.state(), AppState::SessionIdle);

        let mut ui = UiState::default();
        assert!(apply_user_action(
            &model,
            &mut ui,
            UserAction::OpenFileSelection
        ));
        let Some(Overlay::FileSelection(files)) = ui.overlay() else {
            panic!("the review overlay should open");
        };
        assert_eq!(files.selection, selection);
    }

    /// Builds `count` named manifest entries for scroll tests.
    fn manifest(count: usize) -> Vec<FileEntry> {
        (0..count)
            .map(|index| FileEntry::new(format!("f{index:02}.bin"), 1))
            .collect()
    }

    /// Starts an outbound transfer of `count` files with the last one current.
    fn active_transfer(count: usize) -> AppModel {
        let names: Vec<String> = manifest(count)
            .into_iter()
            .map(|entry| entry.name)
            .collect();
        let entries: Vec<(&str, u64)> = names.iter().map(|name| (name.as_str(), 1)).collect();
        let mut model = AppModel::for_test(AppState::SessionIdle);
        update(
            &mut model,
            AppEvent::User(UserAction::StartTransfer(FileSelection::for_test(&entries))),
        );
        update(&mut model, AppEvent::TransferStarted);
        update(
            &mut model,
            AppEvent::TransferProgress(TransferProgress {
                index: u16::try_from(count - 1).unwrap(),
                files: u16::try_from(count).unwrap(),
                file_size: 1,
                transferred: 0,
                total_size: count as u64,
                total_transferred: 0,
                elapsed: std::time::Duration::ZERO,
            }),
        );
        model
    }

    #[test]
    fn active_transfer_scroll_pauses_and_resumes_the_live_follow() {
        let model = active_transfer(12);
        let mut ui = UiState::default();
        reconcile(&model, &mut ui);
        assert!(ui.transfer_scroll().follow());

        // A manual scroll starts from the current (last) file and pauses it.
        assert_eq!(apply_key_input(&model, &mut ui, KeyInput::Up), None);
        assert!(!ui.transfer_scroll().follow());
        assert_eq!(ui.transfer_scroll().cursor(), 10);

        // End returns to the live view at the last row.
        assert_eq!(apply_key_input(&model, &mut ui, KeyInput::End), None);
        assert!(ui.transfer_scroll().follow());
        assert_eq!(ui.transfer_scroll().cursor(), 11);

        // Page, home, and row keys stay bounded and never cancel.
        assert_eq!(apply_key_input(&model, &mut ui, KeyInput::PageUp), None);
        assert_eq!(ui.transfer_scroll().cursor(), 1);
        assert_eq!(apply_key_input(&model, &mut ui, KeyInput::Home), None);
        assert_eq!(ui.transfer_scroll().cursor(), 0);
        assert_eq!(apply_key_input(&model, &mut ui, KeyInput::Down), None);
        assert_eq!(ui.transfer_scroll().cursor(), 1);
        assert_eq!(
            apply_key_input(&model, &mut ui, KeyInput::Enter),
            Some(UserAction::CancelTransfer)
        );

        // The next transfer opens following the live file again.
        reconcile(&AppModel::for_test(AppState::SessionIdle), &mut ui);
        let next = active_transfer(4);
        reconcile(&next, &mut ui);
        assert!(ui.transfer_scroll().follow());
        assert_eq!(ui.transfer_scroll().cursor(), 0);
    }

    #[test]
    fn finished_summary_scrolls_and_resets_for_the_next_transfer() {
        let mut model = AppModel::for_test(AppState::TransferringInbound);
        update(
            &mut model,
            AppEvent::TransferCompleted(TransferSummary::new(
                TransferDirection::Received,
                manifest(12),
                None,
                Some("peer".to_owned()),
                std::time::Duration::from_secs(1),
            )),
        );
        let mut ui = UiState::default();
        reconcile(&model, &mut ui);
        assert_eq!(ui.summary_scroll(), 0);

        assert_eq!(apply_key_input(&model, &mut ui, KeyInput::End), None);
        assert_eq!(ui.summary_scroll(), 11);
        assert_eq!(apply_key_input(&model, &mut ui, KeyInput::PageUp), None);
        assert_eq!(ui.summary_scroll(), 1);
        assert_eq!(apply_key_input(&model, &mut ui, KeyInput::Home), None);
        assert_eq!(ui.summary_scroll(), 0);
        assert_eq!(
            apply_key_input(&model, &mut ui, KeyInput::Enter),
            Some(UserAction::DismissSummary)
        );

        // Leaving and re-entering the summary resets the cursor.
        apply_key_input(&model, &mut ui, KeyInput::End);
        assert_eq!(ui.summary_scroll(), 11);
        reconcile(&AppModel::for_test(AppState::SessionIdle), &mut ui);
        reconcile(&model, &mut ui);
        assert_eq!(ui.summary_scroll(), 0);
    }

    #[test]
    fn inbound_review_scrolls_without_touching_the_destination_or_focus() {
        let mut model = AppModel::for_test(AppState::SessionIdle);
        update(
            &mut model,
            AppEvent::IncomingTransferRequest(TransferProposal::new(
                &TransferRequest::new(manifest(12)).unwrap(),
                Some("peer".to_owned()),
            )),
        );
        let mut ui = UiState::default();
        reconcile(&model, &mut ui);
        let original = model.default_destination().display().to_string();

        // Arrow keys scroll the manifest.
        apply_key_input(&model, &mut ui, KeyInput::Down);
        apply_key_input(&model, &mut ui, KeyInput::End);
        let Some(Overlay::TransferReview(review)) = ui.overlay() else {
            panic!("the inbound review must remain open");
        };
        assert_eq!(review.scroll, 11);
        assert_eq!(review.destination, original);

        apply_key_input(&model, &mut ui, KeyInput::Home);
        apply_key_input(&model, &mut ui, KeyInput::PageDown);
        let Some(Overlay::TransferReview(review)) = ui.overlay() else {
            panic!("the inbound review must remain open");
        };
        assert_eq!(review.scroll, 10);

        // Typing still edits the destination and Left/Right still moves focus.
        apply_key_input(&model, &mut ui, KeyInput::Backspace);
        apply_key_input(&model, &mut ui, KeyInput::Character('x'));
        apply_key_input(&model, &mut ui, KeyInput::Right);
        let Some(Overlay::TransferReview(review)) = ui.overlay() else {
            panic!("the inbound review must remain open");
        };
        assert!(review.destination.ends_with('x'));
        assert_eq!(review.focus, DialogFocus::Reject);
        assert_eq!(review.scroll, 10);
    }

    #[test]
    fn page_keys_move_the_device_and_file_review_selections() {
        let names: Vec<String> = (0..12).map(|index| format!("peer-{index:02}")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let model = browsing_with(&refs);
        let mut ui = UiState::default();

        assert_eq!(apply_key_input(&model, &mut ui, KeyInput::End), None);
        assert_eq!(
            ui.device_selection(),
            Some(model.sorted_candidates()[11].id())
        );
        assert_eq!(apply_key_input(&model, &mut ui, KeyInput::PageUp), None);
        assert_eq!(
            ui.device_selection(),
            Some(model.sorted_candidates()[1].id())
        );
        assert_eq!(apply_key_input(&model, &mut ui, KeyInput::Home), None);
        assert_eq!(
            ui.device_selection(),
            Some(model.sorted_candidates()[0].id())
        );

        // The send review pages around its highlighted file.
        let entries: Vec<(&str, u64)> = names.iter().map(|name| (name.as_str(), 1)).collect();
        ui.overlay = Some(Overlay::FileSelection(FileSelectionInput {
            selection: FileSelection::for_test(&entries),
            selected: Some(0),
            ..FileSelectionInput::default()
        }));
        assert_eq!(apply_key_input(&model, &mut ui, KeyInput::End), None);
        let Some(Overlay::FileSelection(files)) = ui.overlay() else {
            panic!("the review overlay should remain open");
        };
        assert_eq!(files.selected, Some(11));
        assert_eq!(apply_key_input(&model, &mut ui, KeyInput::PageUp), None);
        let Some(Overlay::FileSelection(files)) = ui.overlay() else {
            panic!("the review overlay should remain open");
        };
        assert_eq!(files.selected, Some(1));
    }

    #[test]
    fn update_dialog_follows_the_check_and_install_flow() {
        let model = AppModel::for_test(AppState::Home);
        let mut ui = UiState::default();

        // Starting a check opens the dialog but leaves the effect to the model.
        assert!(!apply_user_action(
            &model,
            &mut ui,
            UserAction::CheckForUpdate
        ));
        assert!(matches!(ui.overlay(), Some(Overlay::Update(_))));

        // A running check ignores input so its result is never orphaned.
        assert_eq!(apply_key_input(&model, &mut ui, KeyInput::Escape), None);
        assert!(matches!(ui.overlay(), Some(Overlay::Update(_))));

        apply_update_check(
            &mut ui,
            UpdateCheck::Available {
                current: "0.1.0".to_owned(),
                new: "0.2.0".to_owned(),
            },
        );
        let Some(Overlay::Update(update)) = ui.overlay() else {
            panic!("the update dialog must stay open");
        };
        assert_eq!(
            update.phase,
            UpdatePhase::Available {
                current: "0.1.0".to_owned(),
                new: "0.2.0".to_owned(),
            }
        );

        // Accepting moves the dialog into its installing phase and leaves the
        // install effect to the model.
        assert!(!apply_user_action(&model, &mut ui, UserAction::ApplyUpdate));
        let Some(Overlay::Update(update)) = ui.overlay() else {
            panic!("the update dialog must stay open");
        };
        assert_eq!(
            update.phase,
            UpdatePhase::Installing {
                new: "0.2.0".to_owned(),
            }
        );
        assert_eq!(apply_key_input(&model, &mut ui, KeyInput::Escape), None);

        apply_update_result(&mut ui, Ok("0.2.0".to_owned()));
        let Some(Overlay::Update(update)) = ui.overlay() else {
            panic!("the update dialog must stay open");
        };
        assert_eq!(
            update.phase,
            UpdatePhase::Installed {
                new: "0.2.0".to_owned(),
            }
        );

        // The restart prompt quits only when its primary button is focused.
        assert_eq!(apply_key_input(&model, &mut ui, KeyInput::Right), None);
        assert_eq!(
            apply_key_input(&model, &mut ui, KeyInput::Enter),
            Some(UserAction::DismissUpdate)
        );
        assert!(ui.overlay().is_none());
    }

    #[test]
    fn update_notice_follows_the_latest_check_result() {
        let mut ui = UiState::default();

        // A newer release is remembered even when no dialog is open.
        apply_update_check(
            &mut ui,
            UpdateCheck::Available {
                current: "0.1.0".to_owned(),
                new: "0.2.0".to_owned(),
            },
        );
        assert_eq!(ui.update_available(), Some("0.2.0"));

        // A failed check changes nothing, so the offer survives it.
        apply_update_check(&mut ui, UpdateCheck::Failed("offline".to_owned()));
        assert_eq!(ui.update_available(), Some("0.2.0"));

        // A managed copy that is current clears an older offer.
        apply_update_check(
            &mut ui,
            UpdateCheck::UpToDate {
                current: "0.2.0".to_owned(),
            },
        );
        assert_eq!(ui.update_available(), None);

        // The same holds when the running copy cannot update itself.
        apply_update_check(
            &mut ui,
            UpdateCheck::Available {
                current: "0.1.0".to_owned(),
                new: "0.2.0".to_owned(),
            },
        );
        apply_update_check(&mut ui, UpdateCheck::NotManaged);
        assert_eq!(ui.update_available(), None);

        // Installing the offered version clears it before the restart.
        apply_update_check(
            &mut ui,
            UpdateCheck::Available {
                current: "0.1.0".to_owned(),
                new: "0.2.0".to_owned(),
            },
        );
        apply_update_result(&mut ui, Ok("0.2.0".to_owned()));
        assert_eq!(ui.update_available(), None);

        // A failed install leaves the offer on the home screen.
        apply_update_check(
            &mut ui,
            UpdateCheck::Available {
                current: "0.1.0".to_owned(),
                new: "0.2.0".to_owned(),
            },
        );
        apply_update_result(&mut ui, Err("install failed".to_owned()));
        assert_eq!(ui.update_available(), Some("0.2.0"));
    }
}
