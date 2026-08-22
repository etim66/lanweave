//! User intent and terminal-independent interaction inputs.

/// Maximum length of a host portion in a [`DirectEndpoint`].
pub(crate) const MAX_DIRECT_HOST_BYTES: usize = 255;
/// Maximum number of characters in a direct-address input line.
///
/// A host has at most [`MAX_DIRECT_HOST_BYTES`] bytes, a port at most five
/// digits, and the separator adds one character.
pub(crate) const MAX_DIRECT_ADDRESS_CHARS: usize = MAX_DIRECT_HOST_BYTES + 6;

/// Why a direct-address input is not a usable route.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DirectAddressError {
    MissingHost,
    HostTooLong,
    InvalidHost,
    MissingPort,
    InvalidPort,
    PortOutOfRange,
}

impl DirectAddressError {
    /// Returns a display-safe explanation for the user.
    pub(crate) const fn message(self) -> &'static str {
        match self {
            Self::MissingHost => "Enter a host or IP address",
            Self::HostTooLong => "The host is too long",
            Self::InvalidHost => "The host must not contain spaces or control characters",
            Self::MissingPort => "Enter a port after the colon",
            Self::InvalidPort => "The port must be a number",
            Self::PortOutOfRange => "The port must be between 1 and 65535",
        }
    }
}

/// Identifies a discovery candidate without exposing adapter-specific data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct DeviceId(u64);

#[cfg_attr(not(test), allow(dead_code))]
impl DeviceId {
    /// Wraps an adapter-provided identifier.
    pub(crate) const fn new(value: u64) -> Self {
        Self(value)
    }
}

/// A user-supplied route used when multicast discovery is unavailable.
///
/// Full syntax and platform validation belongs to the device-selection flow.
/// This type establishes the shared connection entry point.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DirectEndpoint {
    host: String,
    port: u16,
}

impl DirectEndpoint {
    /// Builds an endpoint, rejecting empty or oversized hosts and port zero.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn new(host: String, port: u16) -> Option<Self> {
        if host.is_empty() || host.len() > MAX_DIRECT_HOST_BYTES || port == 0 {
            return None;
        }
        Some(Self { host, port })
    }

    /// Parses and validates a `host:port` input line.
    ///
    /// The port is the text after the last colon, so a bare IPv6 literal like
    /// `::1` is treated as a missing port. Hosts must not contain whitespace
    /// or control bytes; ports must be one to five decimal digits.
    pub(crate) fn parse(input: &str) -> Result<Self, DirectAddressError> {
        let (host, port) = input.rsplit_once(':').unwrap_or((input, ""));

        if host.is_empty() {
            return Err(DirectAddressError::MissingHost);
        }
        if host.len() > MAX_DIRECT_HOST_BYTES {
            return Err(DirectAddressError::HostTooLong);
        }
        if host
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
        {
            return Err(DirectAddressError::InvalidHost);
        }

        if port.is_empty() {
            return Err(DirectAddressError::MissingPort);
        }
        if !port.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(DirectAddressError::InvalidPort);
        }
        let port = port
            .parse::<u16>()
            .map_err(|_| DirectAddressError::PortOutOfRange)?;

        Self::new(host.to_owned(), port).ok_or(DirectAddressError::PortOutOfRange)
    }

    /// Returns the host portion of the endpoint.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn host(&self) -> &str {
        &self.host
    }

    /// Returns the port of the endpoint.
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
    OpenDirectAddress,
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

#[cfg(test)]
mod tests {
    use super::{DirectAddressError, DirectEndpoint, MAX_DIRECT_HOST_BYTES};

    #[test]
    fn parse_accepts_valid_host_port_forms() {
        let endpoint = DirectEndpoint::parse("peer.local:4242").unwrap();
        assert_eq!(endpoint.host(), "peer.local");
        assert_eq!(endpoint.port(), 4242);

        let ipv6 = DirectEndpoint::parse("::1:4242").unwrap();
        assert_eq!(ipv6.host(), "::1");
        assert_eq!(ipv6.port(), 4242);

        let host = "x".repeat(MAX_DIRECT_HOST_BYTES);
        let boundary = DirectEndpoint::parse(&format!("{host}:1")).unwrap();
        assert_eq!(boundary.host().len(), MAX_DIRECT_HOST_BYTES);
    }

    #[test]
    fn parse_rejects_every_invalid_shape() {
        let cases = [
            ("", DirectAddressError::MissingHost),
            (":4242", DirectAddressError::MissingHost),
            ("peer.local", DirectAddressError::MissingPort),
            ("peer.local:", DirectAddressError::MissingPort),
            ("peer.local:abc", DirectAddressError::InvalidPort),
            ("peer.local:0", DirectAddressError::PortOutOfRange),
            ("peer.local:65536", DirectAddressError::PortOutOfRange),
            ("peer.local:123456", DirectAddressError::PortOutOfRange),
            ("peer local:4242", DirectAddressError::InvalidHost),
            ("peer\tlocal:4242", DirectAddressError::InvalidHost),
        ];

        for (input, expected) in cases {
            assert_eq!(
                DirectEndpoint::parse(input),
                Err(expected),
                "input: {input:?}"
            );
        }

        assert_eq!(
            DirectEndpoint::parse(&format!("{}:4242", "x".repeat(MAX_DIRECT_HOST_BYTES + 1))),
            Err(DirectAddressError::HostTooLong)
        );
    }
}
