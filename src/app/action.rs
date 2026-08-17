//! User intent and terminal-independent interaction inputs.

pub(crate) const MAX_DIRECT_HOST_BYTES: usize = 255;

/// Identifies a discovery candidate without exposing adapter-specific data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct DeviceId(u64);

#[cfg_attr(not(test), allow(dead_code))]
impl DeviceId {
    pub(crate) const fn new(value: u64) -> Self {
        Self(value)
    }
}

/// A user-supplied route used when multicast discovery is unavailable.
///
/// Full syntax and platform validation belongs to the device-selection flow in
/// PR 7. This type establishes the shared connection entry point now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DirectEndpoint {
    host: String,
    port: u16,
}

impl DirectEndpoint {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn new(host: String, port: u16) -> Option<Self> {
        if host.is_empty() || host.len() > MAX_DIRECT_HOST_BYTES || port == 0 {
            return None;
        }
        Some(Self { host, port })
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn host(&self) -> &str {
        &self.host
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) const fn port(&self) -> u16 {
        self.port
    }
}

/// A route selected from discovery or entered directly by the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConnectionTarget {
    Discovered(DeviceId),
    Direct(DirectEndpoint),
}

/// User intent produced by the TUI or command registry.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) enum UserAction {
    ShowHelp,
    ShowDevices,
    SelectDevice(DeviceId),
    ConnectDirect(DirectEndpoint),
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
