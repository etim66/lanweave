//! Strict JSON control message validation and the protocol state machine.
//!
//! Defines the version 1 message schemas (`docs/MESSAGE_FORMAT.md`), the
//! protocol state validation rules (`docs/STATE_MACHINES.md`), and the fixed
//! wire limits (`docs/PROTOCOL.md`). The frame codec lives alongside in
//! [`crate::framing`]; the transport wires both to sockets in a later PR, so
//! nothing here touches TLS or file I/O.
//!
//! Callers retain the exact body bytes of `hello` frames for the later
//! pairing binding; the frame body is the authoritative byte source.
//!
//! Items are `pub` inside this private module so the fuzz shim can re-export
//! the codecs; the module boundary keeps them crate-internal. The layer is
//! exercised by unit tests until the transport integrates it.
#![cfg_attr(not(test), allow(dead_code))]

mod message;
mod state;
mod strict;

// The re-exports below are the module's API surface for the transport
// integration in a later PR, so they are unused for now.
#[allow(unused_imports)]
pub use message::{
    CancelCode, CloseCode, Control, ErrorCode, ErrorMessage, FileEnd, FileEntry, FileFailure,
    FileResult, FileStatus, Hello, MessageError, PairRejection, PairResponse, PairingRecord,
    PairingStep, TransferCancel, TransferRejection, TransferRequest, TransferResponse,
};
#[allow(unused_imports)]
pub use state::{
    Inbound, Phase, ProtocolAction, ProtocolError, ProtocolState, Role, accept, send, send_data,
};

/// The only accepted protocol version.
pub const PROTOCOL_VERSION: u8 = 1;

/// Generic JSON body limit: 1 MiB.
pub const MAX_JSON_BODY_BYTES: usize = 1_048_576;
/// Exact `hello` body limit.
pub const MAX_HELLO_BODY_BYTES: usize = 4_000;
/// `pairing` body limit.
pub const MAX_PAIRING_BODY_BYTES: usize = 4_096;
/// `transfer_request` body limit: 256 KiB.
pub const MAX_TRANSFER_REQUEST_BODY_BYTES: usize = 262_144;
/// Largest allowed protocol integer: `2^53-1`.
pub const MAX_INTEGER: u64 = (1 << 53) - 1;
/// Maximum manifest entries; valid counts are `1..=MAX_FILES`.
pub const MAX_FILES: u16 = 1_024;
/// Highest valid manifest index.
pub const MAX_FILE_INDEX: u16 = MAX_FILES - 1;
/// Maximum filename length in bytes.
pub const MAX_NAME_BYTES: usize = 255;
/// Maximum `hello` display name length in bytes.
pub const MAX_DISPLAY_NAME_BYTES: usize = 128;
/// Exact SPAKE2 share length in bytes for the fixed v1 profile.
pub const SHARE_BYTES: usize = 65;
/// Exact SPAKE2 confirmation tag length in bytes for the fixed v1 profile.
pub const CONFIRM_BYTES: usize = 32;
