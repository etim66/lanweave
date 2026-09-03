//! Incremental bounded frame codec for JSON controls and binary DATA frames.
//!
//! Implements the fixed 12-byte header (see `docs/MESSAGE_FORMAT.md`) and
//! handles partial and coalesced socket reads without unbounded allocation.
//! Bodies stay as exact bytes: JSON controls are parsed strictly only by
//! [`crate::protocol`], and `DATA` reaches the transfer sink untouched.
//!
//! Items are `pub` inside this private module so the fuzz shim can re-export
//! the codec; the module boundary keeps it crate-internal. The transport
//! wires it to sockets in a later PR, so it is exercised by unit tests
//! meanwhile.
#![cfg_attr(not(test), allow(dead_code))]

use bytes::{Buf, Bytes, BytesMut};

/// Fixed header length in bytes.
pub const HEADER_LEN: usize = 12;
/// ASCII magic at the start of every frame.
pub const MAGIC: [u8; 4] = *b"LNWV";
/// The only accepted frame version.
pub const FRAME_VERSION: u8 = 1;
/// Frame kind byte for a strict JSON control body.
pub const KIND_CONTROL: u8 = 0;
/// Frame kind byte for raw file bytes.
pub const KIND_DATA: u8 = 1;
/// Maximum JSON control body size: 1 MiB.
pub const MAX_CONTROL_BODY_BYTES: usize = 1_048_576;
/// Maximum `DATA` body size: 1 MiB (valid sizes are `1..=MAX`).
pub const MAX_DATA_BODY_BYTES: usize = 1_048_576;

/// The two frame kinds on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameKind {
    /// One strict JSON control body.
    Control,
    /// Raw bytes for the current file.
    Data,
}

impl FrameKind {
    const fn to_byte(self) -> u8 {
        match self {
            Self::Control => KIND_CONTROL,
            Self::Data => KIND_DATA,
        }
    }

    const fn max_body(self) -> usize {
        match self {
            Self::Control => MAX_CONTROL_BODY_BYTES,
            Self::Data => MAX_DATA_BODY_BYTES,
        }
    }
}

/// One decoded frame with its exact body bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    /// A JSON control body awaiting strict message parsing.
    Control(Bytes),
    /// Raw file bytes for the current transfer.
    Data(Bytes),
}

impl Frame {
    /// Builds a control frame around a JSON body.
    pub const fn control(body: Bytes) -> Self {
        Self::Control(body)
    }

    /// Builds a `DATA` frame around raw file bytes.
    pub const fn data(body: Bytes) -> Self {
        Self::Data(body)
    }

    /// Returns the frame kind.
    pub const fn kind(&self) -> FrameKind {
        match self {
            Self::Control(_) => FrameKind::Control,
            Self::Data(_) => FrameKind::Data,
        }
    }

    /// Consumes the frame into its body bytes.
    pub fn into_body(self) -> Bytes {
        match self {
            Self::Control(body) | Self::Data(body) => body,
        }
    }

    /// Appends the header and body to `dst`, validating sizes first.
    pub fn encode(&self, dst: &mut BytesMut) -> Result<(), FrameError> {
        let kind = self.kind();
        let body: &[u8] = match self {
            Self::Control(body) | Self::Data(body) => body,
        };
        if body.len() > kind.max_body() {
            return Err(FrameError::BodyTooLarge);
        }
        if kind == FrameKind::Data && body.is_empty() {
            return Err(FrameError::EmptyData);
        }

        let mut header = [0; HEADER_LEN];
        header[..4].copy_from_slice(&MAGIC);
        header[4] = FRAME_VERSION;
        header[5] = kind.to_byte();
        header[6..8].copy_from_slice(&u16::try_from(HEADER_LEN).unwrap().to_be_bytes());
        header[8..12].copy_from_slice(
            &u32::try_from(body.len())
                .expect("body length is bounded")
                .to_be_bytes(),
        );
        dst.reserve(HEADER_LEN + body.len());
        dst.extend_from_slice(&header);
        dst.extend_from_slice(body);
        Ok(())
    }
}

/// Why a frame was rejected. All variants close the connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameError {
    /// The magic bytes are not `LNWV`.
    BadMagic,
    /// The frame version is not 1.
    BadVersion,
    /// The kind byte is neither JSON nor `DATA`.
    BadKind,
    /// The header length is not 12.
    BadHeaderLength,
    /// The body length exceeds the kind's limit.
    BodyTooLarge,
    /// A `DATA` frame carries no bytes.
    EmptyData,
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            Self::BadMagic => "bad frame magic",
            Self::BadVersion => "bad frame version",
            Self::BadKind => "unknown frame kind",
            Self::BadHeaderLength => "bad frame header length",
            Self::BodyTooLarge => "frame body exceeds the fixed limit",
            Self::EmptyData => "DATA frame with an empty body",
        };
        formatter.write_str(text)
    }
}

impl std::error::Error for FrameError {}

/// Decodes one frame from the buffer, waiting for more bytes when incomplete.
///
/// The complete header plus body length are validated before any body
/// allocation; oversized lengths fail regardless of how many bytes arrived.
/// Callers feed socket reads into `src` and loop until [`Ok(None)`] to drain
/// coalesced frames.
pub fn decode(src: &mut BytesMut) -> Result<Option<Frame>, FrameError> {
    if src.len() < HEADER_LEN {
        return Ok(None);
    }
    if src[..4] != MAGIC {
        return Err(FrameError::BadMagic);
    }
    if src[4] != FRAME_VERSION {
        return Err(FrameError::BadVersion);
    }
    let kind = match src[5] {
        KIND_CONTROL => FrameKind::Control,
        KIND_DATA => FrameKind::Data,
        _ => return Err(FrameError::BadKind),
    };
    let header_len = u16::from_be_bytes([src[6], src[7]]);
    if usize::from(header_len) != HEADER_LEN {
        return Err(FrameError::BadHeaderLength);
    }
    let body_len = u32::from_be_bytes([src[8], src[9], src[10], src[11]]) as usize;
    if body_len > kind.max_body() {
        return Err(FrameError::BodyTooLarge);
    }
    if kind == FrameKind::Data && body_len == 0 {
        return Err(FrameError::EmptyData);
    }

    let needed = HEADER_LEN + body_len;
    if src.len() < needed {
        // Bounded by the just-validated body limit.
        src.reserve(needed - src.len());
        return Ok(None);
    }
    let mut frame = src.split_to(needed);
    frame.advance(HEADER_LEN);
    let body = frame.freeze();
    Ok(Some(match kind {
        FrameKind::Control => Frame::Control(body),
        FrameKind::Data => Frame::Data(body),
    }))
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use super::{
        Frame, FrameError, HEADER_LEN, MAX_CONTROL_BODY_BYTES, MAX_DATA_BODY_BYTES, decode,
    };

    fn control_frame(body: &[u8]) -> Frame {
        Frame::control(Bytes::copy_from_slice(body))
    }

    fn data_frame(body: &[u8]) -> Frame {
        Frame::data(Bytes::copy_from_slice(body))
    }

    fn decode_all(buffer: &mut bytes::BytesMut) -> Result<Vec<Frame>, FrameError> {
        let mut frames = Vec::new();
        while let Some(frame) = decode(buffer)? {
            frames.push(frame);
        }
        Ok(frames)
    }

    fn header(kind: u8, body_len: u32) -> [u8; 12] {
        let mut header = [0; 12];
        header[..4].copy_from_slice(b"LNWV");
        header[4] = 1;
        header[5] = kind;
        header[6..8].copy_from_slice(&12u16.to_be_bytes());
        header[8..12].copy_from_slice(&body_len.to_be_bytes());
        header
    }

    #[test]
    fn frames_decode_across_every_split_point() {
        let frames = [
            control_frame(br#"{"type":"hello","version":1}"#),
            data_frame(b"file-bytes"),
            control_frame(br#"{"type":"ready"}"#),
            data_frame(&[0xAB; 4096]),
        ];
        let mut wire = bytes::BytesMut::new();
        for frame in &frames {
            frame.encode(&mut wire).unwrap();
        }

        for split in 0..=wire.len() {
            let mut buffer = bytes::BytesMut::new();
            buffer.extend_from_slice(&wire[..split]);
            let mut decoded = decode_all(&mut buffer).unwrap();
            buffer.extend_from_slice(&wire[split..]);
            decoded.extend(decode_all(&mut buffer).unwrap());
            assert_eq!(decoded, frames, "split at {split}");
            assert!(buffer.is_empty());
        }
    }

    #[test]
    fn byte_by_byte_feeding_decodes_identically() {
        let frames = [
            control_frame(br#"{"type":"pair_request"}"#),
            data_frame(&[1; 1024]),
        ];
        let mut wire = bytes::BytesMut::new();
        for frame in &frames {
            frame.encode(&mut wire).unwrap();
        }

        let mut buffer = bytes::BytesMut::new();
        let mut decoded = Vec::new();
        for byte in wire.as_ref() {
            buffer.extend_from_slice(&[*byte]);
            decoded.extend(decode_all(&mut buffer).unwrap());
        }
        assert_eq!(decoded, frames);
    }

    #[test]
    fn short_buffers_wait_for_more_bytes() {
        let frame = control_frame(br#"{"type":"ready"}"#);
        let mut wire = bytes::BytesMut::new();
        frame.encode(&mut wire).unwrap();

        // Every partial header length waits.
        for length in 0..HEADER_LEN {
            let mut buffer = bytes::BytesMut::new();
            buffer.extend_from_slice(&wire[..length]);
            assert_eq!(decode(&mut buffer), Ok(None), "header length {length}");
        }

        // A complete header with a partial body waits.
        let mut buffer = bytes::BytesMut::new();
        buffer.extend_from_slice(&wire[..HEADER_LEN + 3]);
        assert_eq!(decode(&mut buffer), Ok(None));

        // Completing the body yields the frame with its exact body bytes.
        buffer.extend_from_slice(&wire[HEADER_LEN + 3..]);
        let mut decoded = decode_all(&mut buffer).unwrap();
        assert_eq!(decoded.len(), 1);
        assert_eq!(
            decoded.remove(0).into_body(),
            Bytes::from_static(br#"{"type":"ready"}"#)
        );
    }

    #[test]
    fn invalid_headers_fail_before_body_allocation() {
        let oversized = (MAX_CONTROL_BODY_BYTES + 1) as u32;
        let cases: Vec<([u8; 12], FrameError)> = vec![
            (
                {
                    let mut header = header(0, 2);
                    header[..4].copy_from_slice(b"XXXX");
                    header
                },
                FrameError::BadMagic,
            ),
            (
                {
                    let mut header = header(0, 2);
                    header[4] = 0;
                    header
                },
                FrameError::BadVersion,
            ),
            (
                {
                    let mut header = header(0, 2);
                    header[4] = 2;
                    header
                },
                FrameError::BadVersion,
            ),
            (
                {
                    let mut header = header(0, 2);
                    header[5] = 7;
                    header
                },
                FrameError::BadKind,
            ),
            (
                {
                    let mut header = header(0, 2);
                    header[6..8].copy_from_slice(&11u16.to_be_bytes());
                    header
                },
                FrameError::BadHeaderLength,
            ),
            (header(0, oversized), FrameError::BodyTooLarge),
            (header(0, u32::MAX), FrameError::BodyTooLarge),
            (
                header(1, (MAX_DATA_BODY_BYTES + 1) as u32),
                FrameError::BodyTooLarge,
            ),
            (header(1, 0), FrameError::EmptyData),
        ];

        for (header, expected) in cases {
            let mut buffer = bytes::BytesMut::new();
            buffer.extend_from_slice(&header);
            buffer.extend_from_slice(b"body");
            assert_eq!(decode(&mut buffer), Err(expected), "header: {header:?}");
        }
    }

    #[test]
    fn encode_rejects_out_of_bounds_bodies() {
        let mut dst = bytes::BytesMut::new();
        assert_eq!(data_frame(&[]).encode(&mut dst), Err(FrameError::EmptyData));
        assert_eq!(
            control_frame(&vec![0; MAX_CONTROL_BODY_BYTES + 1]).encode(&mut dst),
            Err(FrameError::BodyTooLarge)
        );
        assert_eq!(
            data_frame(&vec![0; MAX_DATA_BODY_BYTES + 1]).encode(&mut dst),
            Err(FrameError::BodyTooLarge)
        );
        assert!(dst.is_empty());
    }
}
