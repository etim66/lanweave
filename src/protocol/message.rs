//! Version 1 control message values with strict JSON decoding.
//!
//! Every message follows `docs/MESSAGE_FORMAT.md`: one UTF-8 JSON object that
//! rejects duplicate and unknown fields, closed string enums, bounded strings
//! and arrays, and integers from `0` through `2^53-1`. Encoding emits the
//! documented field order so golden fixtures stay byte-stable. Peer-provided
//! text never reaches errors or reason values.

use std::fmt;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Serialize;

use super::strict::StrictObject;
use super::{
    CONFIRM_BYTES, MAX_DISPLAY_NAME_BYTES, MAX_FILE_INDEX, MAX_FILES, MAX_HELLO_BODY_BYTES,
    MAX_INTEGER, MAX_JSON_BODY_BYTES, MAX_NAME_BYTES, MAX_PAIRING_BODY_BYTES,
    MAX_TRANSFER_REQUEST_BODY_BYTES, PROTOCOL_VERSION, SHARE_BYTES,
};

/// Why a control body was rejected.
///
/// Variants carry only static schema text, never peer-controlled bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageError {
    /// The body is not valid JSON.
    Malformed,
    /// The body is valid JSON but not a single object.
    NotAnObject,
    /// A schema field appears more than once.
    DuplicateField(&'static str),
    /// A field outside the message schema.
    UnknownField,
    /// A required schema field is missing.
    MissingField(&'static str),
    /// A field has the wrong JSON type or shape.
    WrongType {
        /// The schema field name.
        field: &'static str,
        /// Static description of the expected JSON shape.
        expected: &'static str,
    },
    /// A field violates a fixed limit or closed value set.
    InvalidValue {
        /// The schema field name.
        field: &'static str,
    },
    /// The body exceeds the message size limit.
    BodyTooLarge {
        /// The limit in bytes.
        limit: usize,
    },
}

impl fmt::Display for MessageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed => formatter.write_str("malformed JSON body"),
            Self::NotAnObject => formatter.write_str("body is not a JSON object"),
            Self::DuplicateField(field) => write!(formatter, "duplicate field `{field}`"),
            Self::UnknownField => formatter.write_str("unknown field"),
            Self::MissingField(field) => write!(formatter, "missing field `{field}`"),
            Self::WrongType { field, expected } => {
                write!(formatter, "field `{field}` is not {expected}")
            }
            Self::InvalidValue { field } => {
                write!(formatter, "field `{field}` violates a fixed limit")
            }
            Self::BodyTooLarge { limit } => {
                write!(formatter, "body exceeds the {limit} byte limit")
            }
        }
    }
}

impl std::error::Error for MessageError {}

/// Defines a closed wire enum with a fixed set of string values.
macro_rules! wire_enum {
    ($(#[$doc:meta])* $name:ident { $($variant:ident => $wire:literal),+ $(,)? }) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name {
            $($variant),+
        }

        impl $name {
            /// Parses the closed wire value set; other peer text is rejected.
            pub fn from_wire(text: &str) -> Option<Self> {
                match text {
                    $($wire => Some(Self::$variant),)+
                    _ => None,
                }
            }
        }
    };
}

wire_enum!(
    /// Closed rejection reasons for `pair_response`.
    PairRejection {
        UserRejected => "user_rejected",
        Busy => "busy",
        Timeout => "timeout",
        Unavailable => "unavailable",
    }
);

wire_enum!(
    /// Closed rejection reasons for `transfer_response`.
    TransferRejection {
        UserRejected => "user_rejected",
        Busy => "busy",
        Timeout => "timeout",
        InvalidManifest => "invalid_manifest",
        InvalidFilename => "invalid_filename",
        NameConflict => "name_conflict",
        DestinationExists => "destination_exists",
        InsufficientStorage => "insufficient_storage",
        ResourceLimit => "resource_limit",
        Unavailable => "unavailable",
    }
);

wire_enum!(
    /// Closed failure codes for a failed `file_result`.
    FileFailure {
        SizeMismatch => "size_mismatch",
        HashMismatch => "hash_mismatch",
        WriteFailed => "write_failed",
        DestinationExists => "destination_exists",
        InsufficientStorage => "insufficient_storage",
    }
);

wire_enum!(
    /// Closed cancel codes for `transfer_cancel`.
    CancelCode {
        UserCancelled => "user_cancelled",
        SourceUnavailable => "source_unavailable",
    }
);

wire_enum!(
    /// Closed close codes for `session_close`.
    CloseCode {
        UserClosed => "user_closed",
        IdleTimeout => "idle_timeout",
        Shutdown => "shutdown",
    }
);

wire_enum!(
    /// Closed error codes for `error`.
    ErrorCode {
        UnsupportedVersion => "unsupported_version",
        InvalidMessage => "invalid_message",
        AuthenticationFailed => "authentication_failed",
        Timeout => "timeout",
        ResourceLimit => "resource_limit",
        InternalError => "internal_error",
    }
);

wire_enum!(
    /// Which pairing record a `pairing` message carries.
    PairingStep {
        Share => "share",
        Confirm => "confirm",
    }
);

wire_enum!(
    /// Whether a verified file completed or failed.
    FileStatus {
        Verified => "verified",
        Failed => "failed",
    }
);

/// One version 1 control message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Control {
    /// The first exchanged message on a connection.
    Hello(Hello),
    /// The initiator asks the responder to show a pairing prompt.
    PairRequest,
    /// The responder's accept or reject decision.
    PairResponse(PairResponse),
    /// One SPAKE2 share or confirmation record.
    Pairing(PairingRecord),
    /// The immutable ordered manifest proposal.
    TransferRequest(TransferRequest),
    /// The recipient's accept or reject decision.
    TransferResponse(TransferResponse),
    /// The recipient is prepared and the requester may send `DATA`.
    Ready,
    /// One file finished streaming with its digest.
    FileEnd(FileEnd),
    /// The recipient's per-file verification outcome.
    FileResult(FileResult),
    /// A participant cancels the current proposal or transfer.
    TransferCancel(TransferCancel),
    /// A participant ends the authorized session.
    SessionClose(SessionClose),
    /// A terminal error report that closes the connection.
    Error(ErrorMessage),
}

/// `hello`: untrusted display text plus the fixed protocol version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Hello {
    pub version: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}

impl Hello {
    /// Creates a hello carrying the fixed protocol version.
    pub fn new(display_name: Option<String>) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            display_name,
        }
    }
}

/// `pair_response`: the pairing decision with a reason only on rejection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PairResponse {
    pub accepted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<PairRejection>,
}

impl PairResponse {
    /// Builds an accepting response without a reason.
    pub const fn accepted() -> Self {
        Self {
            accepted: true,
            reason: None,
        }
    }

    /// Builds a rejecting response with a closed reason.
    pub const fn rejected(reason: PairRejection) -> Self {
        Self {
            accepted: false,
            reason: Some(reason),
        }
    }
}

/// `pairing`: one pairing record with canonical base64url data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PairingRecord {
    pub step: PairingStep,
    #[serde(serialize_with = "serialize_base64url")]
    pub data: Vec<u8>,
}

impl PairingRecord {
    /// Builds a record without validating cryptographic shape.
    pub fn new(step: PairingStep, data: Vec<u8>) -> Self {
        Self { step, data }
    }
}

/// One manifest entry: a filename component and its exact byte size.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileEntry {
    pub name: String,
    pub size: u64,
}

/// `transfer_request`: the immutable ordered manifest proposal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TransferRequest {
    pub files: Vec<FileEntry>,
    /// Checked total of all sizes; validated once, not sent on the wire.
    #[serde(skip_serializing)]
    pub total_size: u64,
}

impl TransferRequest {
    /// Builds a request, rejecting totals beyond the protocol integer bound.
    pub fn new(files: Vec<FileEntry>) -> Option<Self> {
        let mut total_size = 0u64;
        for entry in &files {
            total_size = total_size
                .checked_add(entry.size)
                .filter(|total| *total <= MAX_INTEGER)?;
        }
        Some(Self { files, total_size })
    }
}

/// `transfer_response`: the transfer decision with a reason only on rejection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TransferResponse {
    pub accepted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<TransferRejection>,
}

impl TransferResponse {
    /// Builds an accepting response without a reason.
    pub const fn accepted() -> Self {
        Self {
            accepted: true,
            reason: None,
        }
    }

    /// Builds a rejecting response with a closed reason.
    pub const fn rejected(reason: TransferRejection) -> Self {
        Self {
            accepted: false,
            reason: Some(reason),
        }
    }
}

/// `file_end`: the manifest index and digest of one streamed file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileEnd {
    pub index: u16,
    #[serde(serialize_with = "serialize_sha256")]
    pub sha256: [u8; 32],
}

/// `file_result`: a per-file outcome with a code only on failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileResult {
    pub index: u16,
    pub status: FileStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<FileFailure>,
}

impl FileResult {
    /// Builds a verified result without a code.
    pub const fn verified(index: u16) -> Self {
        Self {
            index,
            status: FileStatus::Verified,
            code: None,
        }
    }

    /// Builds a failed result carrying a closed code.
    pub const fn failed(index: u16, code: FileFailure) -> Self {
        Self {
            index,
            status: FileStatus::Failed,
            code: Some(code),
        }
    }
}

/// `transfer_cancel`: a closed cancel code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TransferCancel {
    pub code: CancelCode,
}

/// `session_close`: a closed close code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionClose {
    pub code: CloseCode,
}

/// `error`: a terminal error report with a closed code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ErrorMessage {
    pub code: ErrorCode,
}

/// Serializes binary pairing data as canonical unpadded base64url.
fn serialize_base64url<S: serde::Serializer>(
    data: &[u8],
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&URL_SAFE_NO_PAD.encode(data))
}

/// Serializes a digest as 64 lowercase hexadecimal characters.
fn serialize_sha256<S: serde::Serializer>(
    digest: &[u8; 32],
    serializer: S,
) -> Result<S::Ok, S::Error> {
    use std::fmt::Write as _;

    let mut hex = String::with_capacity(64);
    for byte in digest {
        write!(hex, "{byte:02x}").expect("string writes cannot fail");
    }
    serializer.serialize_str(&hex)
}

impl Control {
    /// Decodes one strict JSON control body.
    pub fn decode(body: &[u8]) -> Result<Control, MessageError> {
        if body.len() > MAX_JSON_BODY_BYTES {
            return Err(MessageError::BodyTooLarge {
                limit: MAX_JSON_BODY_BYTES,
            });
        }
        let object = StrictObject::parse(body)?;
        match object.required_str("type")?.as_ref() {
            "hello" => Self::decode_hello(body, object),
            "pair_request" => Self::decode_pair_request(object),
            "pair_response" => Self::decode_pair_response(object),
            "pairing" => Self::decode_pairing(body, object),
            "transfer_request" => Self::decode_transfer_request(body, object),
            "transfer_response" => Self::decode_transfer_response(object),
            "ready" => Self::decode_ready(object),
            "file_end" => Self::decode_file_end(object),
            "file_result" => Self::decode_file_result(object),
            "transfer_cancel" => Self::decode_transfer_cancel(object),
            "session_close" => Self::decode_session_close(object),
            "error" => Self::decode_error(object),
            _ => Err(MessageError::InvalidValue { field: "type" }),
        }
    }

    /// Encodes the control as the documented JSON body.
    pub fn encode(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("control serialization cannot fail")
    }

    fn decode_hello(body: &[u8], object: StrictObject<'_>) -> Result<Control, MessageError> {
        if body.len() > MAX_HELLO_BODY_BYTES {
            return Err(MessageError::BodyTooLarge {
                limit: MAX_HELLO_BODY_BYTES,
            });
        }
        let version = object.required_uint("version", MAX_INTEGER)?;
        if version != u64::from(PROTOCOL_VERSION) {
            return Err(MessageError::InvalidValue { field: "version" });
        }
        let display_name = match object.optional_str("display_name")? {
            Some(name) => {
                validate_display_name(&name)?;
                Some(name.into_owned())
            }
            None => None,
        };
        object.finish(&["type", "version", "display_name"])?;
        Ok(Control::Hello(Hello::new(display_name)))
    }

    fn decode_pair_request(object: StrictObject<'_>) -> Result<Control, MessageError> {
        object.finish(&["type"])?;
        Ok(Control::PairRequest)
    }

    fn decode_pair_response(object: StrictObject<'_>) -> Result<Control, MessageError> {
        let accepted = object.required_bool("accepted")?;
        let response = if accepted {
            // A reason is present only for rejection.
            if object.contains("reason") {
                return Err(MessageError::InvalidValue { field: "reason" });
            }
            PairResponse::accepted()
        } else {
            let reason = object.required_str("reason")?;
            let reason = PairRejection::from_wire(&reason)
                .ok_or(MessageError::InvalidValue { field: "reason" })?;
            PairResponse::rejected(reason)
        };
        object.finish(&["type", "accepted", "reason"])?;
        Ok(Control::PairResponse(response))
    }

    fn decode_pairing(body: &[u8], object: StrictObject<'_>) -> Result<Control, MessageError> {
        if body.len() > MAX_PAIRING_BODY_BYTES {
            return Err(MessageError::BodyTooLarge {
                limit: MAX_PAIRING_BODY_BYTES,
            });
        }
        let step = PairingStep::from_wire(&object.required_str("step")?)
            .ok_or(MessageError::InvalidValue { field: "step" })?;
        let expected = match step {
            PairingStep::Share => SHARE_BYTES,
            PairingStep::Confirm => CONFIRM_BYTES,
        };
        let encoded = object.required_str("data")?;
        let data = decode_base64url(&encoded, expected)?;
        object.finish(&["type", "step", "data"])?;
        Ok(Control::Pairing(PairingRecord { step, data }))
    }

    fn decode_transfer_request(
        body: &[u8],
        object: StrictObject<'_>,
    ) -> Result<Control, MessageError> {
        if body.len() > MAX_TRANSFER_REQUEST_BODY_BYTES {
            return Err(MessageError::BodyTooLarge {
                limit: MAX_TRANSFER_REQUEST_BODY_BYTES,
            });
        }
        let entries = object.required_objects("files")?;
        if entries.is_empty() || entries.len() > usize::from(MAX_FILES) {
            return Err(MessageError::InvalidValue { field: "files" });
        }
        let mut files = Vec::with_capacity(entries.len());
        for entry in entries {
            let name = entry.required_str("name")?;
            validate_filename(&name)?;
            let size = entry.required_uint("size", MAX_INTEGER)?;
            entry.finish(&["name", "size"])?;
            files.push(FileEntry {
                name: name.into_owned(),
                size,
            });
        }
        let request =
            TransferRequest::new(files).ok_or(MessageError::InvalidValue { field: "files" })?;
        object.finish(&["type", "files"])?;
        Ok(Control::TransferRequest(request))
    }

    fn decode_transfer_response(object: StrictObject<'_>) -> Result<Control, MessageError> {
        let accepted = object.required_bool("accepted")?;
        let response = if accepted {
            if object.contains("reason") {
                return Err(MessageError::InvalidValue { field: "reason" });
            }
            TransferResponse::accepted()
        } else {
            let reason = object.required_str("reason")?;
            let reason = TransferRejection::from_wire(&reason)
                .ok_or(MessageError::InvalidValue { field: "reason" })?;
            TransferResponse::rejected(reason)
        };
        object.finish(&["type", "accepted", "reason"])?;
        Ok(Control::TransferResponse(response))
    }

    fn decode_ready(object: StrictObject<'_>) -> Result<Control, MessageError> {
        object.finish(&["type"])?;
        Ok(Control::Ready)
    }

    fn decode_file_end(object: StrictObject<'_>) -> Result<Control, MessageError> {
        let index = object.required_uint("index", u64::from(MAX_FILE_INDEX))? as u16;
        let encoded = object.required_str("sha256")?;
        let sha256 = decode_sha256(&encoded)?;
        object.finish(&["type", "index", "sha256"])?;
        Ok(Control::FileEnd(FileEnd { index, sha256 }))
    }

    fn decode_file_result(object: StrictObject<'_>) -> Result<Control, MessageError> {
        let index = object.required_uint("index", u64::from(MAX_FILE_INDEX))? as u16;
        let status = FileStatus::from_wire(&object.required_str("status")?)
            .ok_or(MessageError::InvalidValue { field: "status" })?;
        let result = match status {
            FileStatus::Verified => {
                if object.contains("code") {
                    return Err(MessageError::InvalidValue { field: "code" });
                }
                FileResult::verified(index)
            }
            FileStatus::Failed => {
                let code = object.required_str("code")?;
                let code = FileFailure::from_wire(&code)
                    .ok_or(MessageError::InvalidValue { field: "code" })?;
                FileResult::failed(index, code)
            }
        };
        object.finish(&["type", "index", "status", "code"])?;
        Ok(Control::FileResult(result))
    }

    fn decode_transfer_cancel(object: StrictObject<'_>) -> Result<Control, MessageError> {
        let code = object.required_str("code")?;
        let code =
            CancelCode::from_wire(&code).ok_or(MessageError::InvalidValue { field: "code" })?;
        object.finish(&["type", "code"])?;
        Ok(Control::TransferCancel(TransferCancel { code }))
    }

    fn decode_session_close(object: StrictObject<'_>) -> Result<Control, MessageError> {
        let code = object.required_str("code")?;
        let code =
            CloseCode::from_wire(&code).ok_or(MessageError::InvalidValue { field: "code" })?;
        object.finish(&["type", "code"])?;
        Ok(Control::SessionClose(SessionClose { code }))
    }

    fn decode_error(object: StrictObject<'_>) -> Result<Control, MessageError> {
        let code = object.required_str("code")?;
        let code =
            ErrorCode::from_wire(&code).ok_or(MessageError::InvalidValue { field: "code" })?;
        object.finish(&["type", "code"])?;
        Ok(Control::Error(ErrorMessage { code }))
    }
}

/// Decodes canonical unpadded base64url into exactly `expected_len` bytes.
fn decode_base64url(encoded: &str, expected_len: usize) -> Result<Vec<u8>, MessageError> {
    let decoded = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| MessageError::InvalidValue { field: "data" })?;
    if decoded.len() != expected_len {
        return Err(MessageError::InvalidValue { field: "data" });
    }
    Ok(decoded)
}

/// Decodes 64 lowercase hexadecimal digits into a digest.
fn decode_sha256(encoded: &str) -> Result<[u8; 32], MessageError> {
    let invalid = || MessageError::InvalidValue { field: "sha256" };
    let bytes = encoded.as_bytes();
    if bytes.len() != 64
        || !bytes
            .iter()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    {
        return Err(invalid());
    }
    let (pairs, _) = bytes.as_chunks::<2>();
    let mut digest = [0; 32];
    for (pair, byte) in pairs.iter().zip(&mut digest) {
        let text = std::str::from_utf8(pair).map_err(|_| invalid())?;
        *byte = u8::from_str_radix(text, 16).map_err(|_| invalid())?;
    }
    Ok(digest)
}

/// Rejects names that are not single bounded filename components.
fn validate_filename(name: &str) -> Result<(), MessageError> {
    if is_valid_filename(name) {
        Ok(())
    } else {
        Err(MessageError::InvalidValue { field: "name" })
    }
}

/// Returns whether `name` is a single bounded filename component.
///
/// Shared with local selection so a reviewed name can never be rejected by
/// the wire schema and vice versa.
pub(crate) fn is_valid_filename(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME_BYTES
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !name.contains('\\')
        && !name.chars().any(char::is_control)
}

/// Bounds the untrusted display text length.
fn validate_display_name(name: &str) -> Result<(), MessageError> {
    if name.len() > MAX_DISPLAY_NAME_BYTES {
        return Err(MessageError::InvalidValue {
            field: "display_name",
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;

    use super::{
        CancelCode, CloseCode, Control, ErrorCode, ErrorMessage, FileEnd, FileEntry, FileFailure,
        FileResult, Hello, MessageError, PairRejection, PairResponse, PairingRecord, PairingStep,
        SessionClose, TransferCancel, TransferRejection, TransferRequest, TransferResponse,
        decode_sha256,
    };
    use crate::protocol::{
        MAX_FILES, MAX_HELLO_BODY_BYTES, MAX_INTEGER, MAX_JSON_BODY_BYTES, MAX_NAME_BYTES,
        MAX_TRANSFER_REQUEST_BODY_BYTES,
    };

    /// SHA-256 of the empty string, used as a golden digest.
    const EMPTY_SHA256: [u8; 32] = [
        0xe3, 0xb0, 0xc4, 0x42, 0x98, 0xfc, 0x1c, 0x14, 0x9a, 0xfb, 0xf4, 0xc8, 0x99, 0x6f, 0xb9,
        0x24, 0x27, 0xae, 0x41, 0xe4, 0x64, 0x9b, 0x93, 0x4c, 0xa4, 0x95, 0x99, 0x1b, 0x78, 0x52,
        0xb8, 0x55,
    ];

    /// Golden share value from docs/MESSAGE_FORMAT.md: 65 bytes encoded.
    const SHARE_WIRE: &str =
        "BGsX0fLhLEJH-Lzm5WOkQPJ3A32BLeszoPShOUXYmMKWT-NC4v4af5uO5-tKfA-eFivOM1drMV7Oy7ZAaDe_UfU";

    /// Golden confirmation value: 32 zero bytes encoded.
    const CONFIRM_WIRE: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

    fn golden_messages() -> Vec<(String, Control)> {
        vec![
            (
                "{\"type\":\"hello\",\"version\":1,\"display_name\":\"Workstation\"}".to_owned(),
                Control::Hello(Hello::new(Some("Workstation".to_owned()))),
            ),
            (
                "{\"type\":\"hello\",\"version\":1}".to_owned(),
                Control::Hello(Hello::new(None)),
            ),
            ("{\"type\":\"pair_request\"}".to_owned(), Control::PairRequest),
            (
                "{\"type\":\"pair_response\",\"accepted\":true}".to_owned(),
                Control::PairResponse(PairResponse::accepted()),
            ),
            (
                "{\"type\":\"pair_response\",\"accepted\":false,\"reason\":\"user_rejected\"}"
                    .to_owned(),
                Control::PairResponse(PairResponse::rejected(PairRejection::UserRejected)),
            ),
            (
                format!("{{\"type\":\"pairing\",\"step\":\"share\",\"data\":\"{SHARE_WIRE}\"}}"),
                Control::Pairing(PairingRecord::new(
                    PairingStep::Share,
                    URL_SAFE_NO_PAD.decode(SHARE_WIRE).unwrap(),
                )),
            ),
            (
                format!("{{\"type\":\"pairing\",\"step\":\"confirm\",\"data\":\"{CONFIRM_WIRE}\"}}"),
                Control::Pairing(PairingRecord::new(PairingStep::Confirm, vec![0; 32])),
            ),
            (
                "{\"type\":\"transfer_request\",\"files\":[{\"name\":\"report.pdf\",\"size\":1048576},{\"name\":\"empty.txt\",\"size\":0}]}".to_owned(),
                Control::TransferRequest(
                    TransferRequest::new(vec![
                        FileEntry { name: "report.pdf".to_owned(), size: 1_048_576 },
                        FileEntry { name: "empty.txt".to_owned(), size: 0 },
                    ])
                    .unwrap(),
                ),
            ),
            (
                "{\"type\":\"transfer_request\",\"files\":[{\"name\":\"big.bin\",\"size\":9007199254740991}]}".to_owned(),
                Control::TransferRequest(
                    TransferRequest::new(vec![FileEntry {
                        name: "big.bin".to_owned(),
                        size: MAX_INTEGER,
                    }])
                    .unwrap(),
                ),
            ),
            (
                "{\"type\":\"transfer_response\",\"accepted\":true}".to_owned(),
                Control::TransferResponse(TransferResponse::accepted()),
            ),
            (
                "{\"type\":\"transfer_response\",\"accepted\":false,\"reason\":\"insufficient_storage\"}"
                    .to_owned(),
                Control::TransferResponse(TransferResponse::rejected(
                    TransferRejection::InsufficientStorage,
                )),
            ),
            ("{\"type\":\"ready\"}".to_owned(), Control::Ready),
            (
                "{\"type\":\"file_end\",\"index\":1023,\"sha256\":\"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\"}".to_owned(),
                Control::FileEnd(FileEnd { index: 1023, sha256: EMPTY_SHA256 }),
            ),
            (
                "{\"type\":\"file_result\",\"index\":0,\"status\":\"verified\"}".to_owned(),
                Control::FileResult(FileResult::verified(0)),
            ),
            (
                "{\"type\":\"file_result\",\"index\":1023,\"status\":\"failed\",\"code\":\"hash_mismatch\"}"
                    .to_owned(),
                Control::FileResult(FileResult::failed(1023, FileFailure::HashMismatch)),
            ),
            (
                "{\"type\":\"transfer_cancel\",\"code\":\"source_unavailable\"}".to_owned(),
                Control::TransferCancel(TransferCancel { code: CancelCode::SourceUnavailable }),
            ),
            (
                "{\"type\":\"session_close\",\"code\":\"idle_timeout\"}".to_owned(),
                Control::SessionClose(SessionClose { code: CloseCode::IdleTimeout }),
            ),
            (
                "{\"type\":\"error\",\"code\":\"authentication_failed\"}".to_owned(),
                Control::Error(ErrorMessage { code: ErrorCode::AuthenticationFailed }),
            ),
        ]
    }

    #[test]
    fn golden_messages_round_trip_exactly() {
        for (wire, control) in golden_messages() {
            assert_eq!(control.encode(), wire.as_bytes(), "encode: {control:?}");
            assert_eq!(
                Control::decode(wire.as_bytes()),
                Ok(control),
                "decode: {wire}"
            );
        }
    }

    #[test]
    fn full_manifest_round_trips() {
        let files: Vec<FileEntry> = (0..usize::from(MAX_FILES))
            .map(|index| FileEntry {
                name: format!("f{index:04}"),
                size: 1,
            })
            .collect();
        let control = Control::TransferRequest(TransferRequest::new(files).unwrap());

        assert_eq!(Control::decode(&control.encode()), Ok(control));
    }

    #[test]
    fn strict_bodies_are_rejected() {
        let entries: Vec<String> = (0..=usize::from(MAX_FILES))
            .map(|index| format!("{{\"name\":\"f{index}\",\"size\":1}}"))
            .collect();
        let manifest_1025 = format!(
            "{{\"type\":\"transfer_request\",\"files\":[{}]}}",
            entries.join(",")
        );
        let overflow = "{\"type\":\"transfer_request\",\"files\":[{\"name\":\"a\",\"size\":9007199254740991},{\"name\":\"b\",\"size\":9007199254740991}]}".to_owned();
        let bad_name_length = format!(
            "{{\"type\":\"transfer_request\",\"files\":[{{\"name\":\"{}\",\"size\":1}}]}}",
            "n".repeat(MAX_NAME_BYTES + 1)
        );
        let display_name_129 = format!(
            "{{\"type\":\"hello\",\"version\":1,\"display_name\":\"{}\"}}",
            "x".repeat(129)
        );

        let cases: Vec<(String, MessageError)> = vec![
            // Duplicate fields fail before their values are inspected.
            (
                "{\"type\":\"ready\",\"type\":\"ready\"}".to_owned(),
                MessageError::DuplicateField("type"),
            ),
            (
                "{\"type\":\"hello\",\"version\":1,\"version\":2}".to_owned(),
                MessageError::DuplicateField("version"),
            ),
            (
                "{\"type\":\"pairing\",\"step\":\"share\",\"data\":\"AAAA\",\"data\":\"AAAA\"}"
                    .to_owned(),
                MessageError::DuplicateField("data"),
            ),
            // Unknown and missing fields.
            (
                "{\"type\":\"ready\",\"x\":1}".to_owned(),
                MessageError::UnknownField,
            ),
            (
                "{\"type\":\"hello\",\"version\":1,\"extra\":null}".to_owned(),
                MessageError::UnknownField,
            ),
            ("{\"type\":\"hello\"}".to_owned(), MessageError::MissingField("version")),
            (
                "{\"type\":\"pair_response\",\"accepted\":false}".to_owned(),
                MessageError::MissingField("reason"),
            ),
            (
                "{\"type\":\"file_result\",\"index\":0,\"status\":\"failed\"}".to_owned(),
                MessageError::MissingField("code"),
            ),
            // Wrong JSON types, including bad numbers.
            (
                "{\"type\":\"hello\",\"version\":\"1\"}".to_owned(),
                MessageError::WrongType { field: "version", expected: "an unsigned integer" },
            ),
            (
                "{\"type\":\"hello\",\"version\":1.0}".to_owned(),
                MessageError::WrongType { field: "version", expected: "an unsigned integer" },
            ),
            (
                "{\"type\":\"hello\",\"version\":1e0}".to_owned(),
                MessageError::WrongType { field: "version", expected: "an unsigned integer" },
            ),
            (
                "{\"type\":\"hello\",\"version\":-1}".to_owned(),
                MessageError::WrongType { field: "version", expected: "an unsigned integer" },
            ),
            (
                "{\"type\":\"pair_response\",\"accepted\":\"yes\"}".to_owned(),
                MessageError::WrongType { field: "accepted", expected: "true or false" },
            ),
            (
                "{\"type\":\"transfer_request\",\"files\":\"many\"}".to_owned(),
                MessageError::WrongType { field: "files", expected: "an array of objects" },
            ),
            (
                "{\"type\":\"transfer_request\",\"files\":[{\"name\":\"a\",\"size\":\"1\"}]}".to_owned(),
                MessageError::WrongType { field: "size", expected: "an unsigned integer" },
            ),
            // Values beyond fixed limits or outside closed sets.
            (
                "{\"type\":\"hello\",\"version\":9007199254740992}".to_owned(),
                MessageError::InvalidValue { field: "version" },
            ),
            (display_name_129, MessageError::InvalidValue { field: "display_name" }),
            (
                "{\"type\":\"pair_response\",\"accepted\":true,\"reason\":\"user_rejected\"}"
                    .to_owned(),
                MessageError::InvalidValue { field: "reason" },
            ),
            (
                "{\"type\":\"pair_response\",\"accepted\":false,\"reason\":\"whatever\"}".to_owned(),
                MessageError::InvalidValue { field: "reason" },
            ),
            (
                "{\"type\":\"pairing\",\"step\":\"exchange\",\"data\":\"AAAA\"}".to_owned(),
                MessageError::InvalidValue { field: "step" },
            ),
            (
                format!("{{\"type\":\"pairing\",\"step\":\"share\",\"data\":\"{SHARE_WIRE}=\"}}"),
                MessageError::InvalidValue { field: "data" },
            ),
            (
                "{\"type\":\"pairing\",\"step\":\"share\",\"data\":\"AAAA\"}".to_owned(),
                MessageError::InvalidValue { field: "data" },
            ),
            (
                "{\"type\":\"pairing\",\"step\":\"share\",\"data\":\"!!!!\"}".to_owned(),
                MessageError::InvalidValue { field: "data" },
            ),
            ("{\"type\":\"transfer_request\",\"files\":[]}".to_owned(), {
                MessageError::InvalidValue { field: "files" }
            }),
            (manifest_1025, MessageError::InvalidValue { field: "files" }),
            (overflow, MessageError::InvalidValue { field: "files" }),
            (
                "{\"type\":\"transfer_request\",\"files\":[{\"name\":\"a/b\",\"size\":1}]}".to_owned(),
                MessageError::InvalidValue { field: "name" },
            ),
            (
                "{\"type\":\"transfer_request\",\"files\":[{\"name\":\"..\",\"size\":1}]}".to_owned(),
                MessageError::InvalidValue { field: "name" },
            ),
            (
                "{\"type\":\"transfer_request\",\"files\":[{\"name\":\"a\\u0000b\",\"size\":1}]}".to_owned(),
                MessageError::InvalidValue { field: "name" },
            ),
            (bad_name_length, MessageError::InvalidValue { field: "name" }),
            (
                "{\"type\":\"file_end\",\"index\":1024,\"sha256\":\"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\"}".to_owned(),
                MessageError::InvalidValue { field: "index" },
            ),
            (
                "{\"type\":\"file_end\",\"index\":0,\"sha256\":\"E3B0C44298FC1C149AFBF4C8996FB92427AE41E4649B934CA495991B7852B855\"}".to_owned(),
                MessageError::InvalidValue { field: "sha256" },
            ),
            (
                "{\"type\":\"file_end\",\"index\":0,\"sha256\":\"e3b0\"}".to_owned(),
                MessageError::InvalidValue { field: "sha256" },
            ),
            (
                "{\"type\":\"file_result\",\"index\":0,\"status\":\"verified\",\"code\":\"hash_mismatch\"}"
                    .to_owned(),
                MessageError::InvalidValue { field: "code" },
            ),
            (
                "{\"type\":\"file_result\",\"index\":0,\"status\":\"ok\"}".to_owned(),
                MessageError::InvalidValue { field: "status" },
            ),
            (
                "{\"type\":\"transfer_cancel\",\"code\":\"peer_cancelled\"}".to_owned(),
                MessageError::InvalidValue { field: "code" },
            ),
            (
                "{\"type\":\"session_close\",\"code\":\"later\"}".to_owned(),
                MessageError::InvalidValue { field: "code" },
            ),
            ("{\"type\":\"error\",\"code\":\"nope\"}".to_owned(), {
                MessageError::InvalidValue { field: "code" }
            }),
            ("{\"type\":\"nope\"}".to_owned(), MessageError::InvalidValue { field: "type" }),
            // Structure: objects only, no trailing values, valid UTF-8.
            ("[]".to_owned(), MessageError::NotAnObject),
            ("null".to_owned(), MessageError::NotAnObject),
            ("\"hello\"".to_owned(), MessageError::NotAnObject),
            ("42".to_owned(), MessageError::NotAnObject),
            ("{\"type\":\"ready\"".to_owned(), MessageError::Malformed),
            ("{\"type\":\"ready\"} trailing".to_owned(), MessageError::Malformed),
        ];

        for (body, expected) in cases {
            assert_eq!(
                Control::decode(body.as_bytes()),
                Err(expected),
                "body: {body}"
            );
        }

        // Invalid UTF-8 never reaches schema checks.
        assert_eq!(
            Control::decode(b"{\"type\":\"\xFF\xFE\"}"),
            Err(MessageError::Malformed)
        );
    }

    #[test]
    fn oversized_bodies_fail_before_field_checks() {
        let hello = format!(
            "{{\"type\":\"hello\",\"version\":1,\"display_name\":\"{}\"}}",
            "x".repeat(4_000)
        );
        assert_eq!(
            Control::decode(hello.as_bytes()),
            Err(MessageError::BodyTooLarge {
                limit: MAX_HELLO_BODY_BYTES
            })
        );

        let entries: Vec<String> = (0..usize::from(MAX_FILES))
            .map(|_| format!("{{\"name\":\"{}\",\"size\":1}}", "n".repeat(MAX_NAME_BYTES)))
            .collect();
        let request = format!(
            "{{\"type\":\"transfer_request\",\"files\":[{}]}}",
            entries.join(",")
        );
        assert_eq!(
            Control::decode(request.as_bytes()),
            Err(MessageError::BodyTooLarge {
                limit: MAX_TRANSFER_REQUEST_BODY_BYTES
            })
        );

        let huge = vec![b' '; MAX_JSON_BODY_BYTES + 1];
        assert_eq!(
            Control::decode(&huge),
            Err(MessageError::BodyTooLarge {
                limit: MAX_JSON_BODY_BYTES
            })
        );
    }

    #[test]
    fn digests_must_be_sixtyfour_lowercase_hex_digits() {
        let digest =
            decode_sha256("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")
                .unwrap();
        assert_eq!(digest[0], 0xe3);
        assert_eq!(digest[31], 0x55);
    }
}
