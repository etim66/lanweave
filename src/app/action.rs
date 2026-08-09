//! User intent and terminal-independent interaction inputs.

/// Identifies a discovery candidate without exposing adapter-specific data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct DeviceId(u64);

#[cfg_attr(not(test), allow(dead_code))]
impl DeviceId {
    pub(crate) const fn new(value: u64) -> Self {
        Self(value)
    }
}

/// User intent produced by the TUI or command registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) enum UserAction {
    ShowHelp,
    ShowDevices,
    SelectDevice(DeviceId),
    AcceptPairing,
    RejectPairing,
    StartTransfer,
    AcceptTransfer,
    RejectTransfer,
    Disconnect,
    Quit,
}

/// Terminal-independent keyboard input interpreted against current UI state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyInput {
    Character(char),
    Backspace,
    Enter,
    Escape,
    Up,
    Down,
}
