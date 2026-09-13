//! Pairing adapter wrapping an RFC 9382 SPAKE2-P256-SHA256-HKDF-HMAC library.
//!
//! Lanweave does not implement elliptic-curve group arithmetic itself. The
//! prototype adapter sits behind a strict audit gate and is wired into the
//! mutual confirmation flow bound to the current TLS exporter and exact
//! hello bodies.
//!
//! The selected `pakery-spake2`/`pakery-crypto` crates are new and unaudited;
//! `docs/CRYPTOGRAPHY.md` keeps the release gate open. The code generation,
//! exchange order, and one-attempt rules in this module are what the protocol
//! state machine in [`crate::protocol`] expects.

use std::fmt;

use pakery_core::crypto::CpaceGroup;
use pakery_crypto::P256Group;
use pakery_spake2::{PartyA, PartyAState, PartyB, PartyBState, Spake2Output};
use rand_core::CryptoRng;
use zeroize::Zeroizing;

use crate::protocol::{CONFIRM_BYTES, MAX_HELLO_BODY_BYTES};
use crate::transport::EXPORTER_LEN;

/// Number of decimal digits in a pairing code.
pub(crate) const CODE_DIGITS: usize = 8;
/// Number of distinct pairing codes (`10^8`).
const CODE_RANGE: u32 = 100_000_000;

/// Fixed SPAKE2 Party A identity for protocol version 1.
pub(crate) const IDENTITY_INITIATOR: &[u8] = b"lanweave-v1-initiator";
/// Fixed SPAKE2 Party B identity for protocol version 1.
pub(crate) const IDENTITY_RESPONDER: &[u8] = b"lanweave-v1-responder";
/// Domain-separation label for the exporter and hello binding.
const BINDING_LABEL: &[u8] = b"lanweave-v1-pairing";

/// The RFC 9382 P-256 ciphersuite used by version 1.
pub(crate) type P256Suite = pakery_crypto::Spake2P256;
/// Password scalar type of the P-256 group.
type PairingScalar = <P256Group as CpaceGroup>::Scalar;

/// A safe pairing failure that never carries peer bytes or the code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PairingError {
    /// The local code or binding inputs have an invalid shape.
    InvalidInput,
    /// The peer's share or confirmation failed validation.
    Authentication,
    /// The adapter rejected a local operation.
    Adapter,
}

impl fmt::Display for PairingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::InvalidInput => "invalid pairing input",
            Self::Authentication => "pairing confirmation failed",
            Self::Adapter => "pairing adapter failed",
        };
        formatter.write_str(text)
    }
}

impl std::error::Error for PairingError {}

/// An eight-digit pairing code kept only in memory.
///
/// The digits are zeroized on drop, `Debug` never reveals them, and there is
/// no `Display` implementation so logging or formatting falls back to the
/// redacted `Debug` form.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct PairingCode(Zeroizing<[u8; CODE_DIGITS]>);

impl PairingCode {
    /// Generates a uniform code with leading zeroes preserved.
    ///
    /// The operating-system generator is sampled with rejection sampling so
    /// every value in `00000000..=99999999` has the same probability.
    pub(crate) fn generate(rng: &mut impl CryptoRng) -> Self {
        // Largest multiple of the code range that fits in 32 bits.
        const LIMIT: u64 = (1u64 << 32) - (1u64 << 32) % CODE_RANGE as u64;

        let value = loop {
            let sample = u64::from(rng.next_u32());
            if sample < LIMIT {
                break (sample % u64::from(CODE_RANGE)) as u32;
            }
        };

        let mut digits = [0u8; CODE_DIGITS];
        let mut value = value;
        for slot in digits.iter_mut().rev() {
            *slot = b'0' + (value % 10) as u8;
            value /= 10;
        }
        Self(Zeroizing::new(digits))
    }

    /// Parses exactly eight ASCII digits, preserving leading zeroes.
    pub(crate) fn parse(text: &str) -> Option<Self> {
        let bytes = text.as_bytes();
        if bytes.len() != CODE_DIGITS || !bytes.iter().all(|byte| byte.is_ascii_digit()) {
            return None;
        }

        let mut digits = [0u8; CODE_DIGITS];
        digits.copy_from_slice(bytes);
        Some(Self(Zeroizing::new(digits)))
    }

    /// Formats the code for local display, grouped as `1234 5678`.
    ///
    /// The returned string is a display copy; callers must not log it.
    pub(crate) fn grouped(&self) -> String {
        let mut display = String::with_capacity(CODE_DIGITS + 1);
        for (index, digit) in self.0.iter().enumerate() {
            if index == CODE_DIGITS / 2 {
                display.push(' ');
            }
            display.push(char::from(*digit));
        }
        display
    }

    /// Derives the RFC 9382 password scalar `w` from the code digits.
    ///
    /// The application-specific mapping is a specialist-review item. This
    /// prototype uses `w = OS2IP(digits)` reduced modulo the group order,
    /// which is injective over the ten-million-value code space and keeps
    /// leading zeroes significant.
    fn password_scalar(&self) -> Result<PairingScalar, PairingError> {
        let mut value = 0u64;
        for digit in self.0.iter() {
            value = value * 10 + u64::from(digit - b'0');
        }

        let mut wide = [0u8; 64];
        wide[56..].copy_from_slice(&value.to_be_bytes());
        P256Group::scalar_from_wide_bytes(&wide).map_err(|_| PairingError::InvalidInput)
    }
}

impl fmt::Debug for PairingCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PairingCode([REDACTED])")
    }
}

/// Builds the additional authenticated data that binds SPAKE2 confirmation
/// to one live TLS connection.
///
/// The inputs are length-prefixed with a big-endian `u32` so no combination
/// of exporter and hello bodies can collide with another. The exact hello
/// bytes are used without reformatting, and both hellos are bounded by the
/// protocol limit before they enter the binding.
pub(crate) fn binding(
    exporter: &[u8],
    initiator_hello: &[u8],
    responder_hello: &[u8],
) -> Result<Zeroizing<Vec<u8>>, PairingError> {
    if exporter.len() != EXPORTER_LEN
        || initiator_hello.len() > MAX_HELLO_BODY_BYTES
        || responder_hello.len() > MAX_HELLO_BODY_BYTES
    {
        return Err(PairingError::InvalidInput);
    }

    let mut binding = Zeroizing::new(Vec::new());
    push_field(&mut binding, BINDING_LABEL);
    push_field(&mut binding, exporter);
    push_field(&mut binding, initiator_hello);
    push_field(&mut binding, responder_hello);
    Ok(binding)
}

/// Starts SPAKE2 Party A (the pairing initiator).
pub(crate) fn start_initiator(
    code: &PairingCode,
    binding: &[u8],
    rng: &mut impl CryptoRng,
) -> Result<(Vec<u8>, PartyAState<P256Suite>), PairingError> {
    let password = code.password_scalar()?;
    PartyA::<P256Suite>::start(
        &password,
        IDENTITY_INITIATOR,
        IDENTITY_RESPONDER,
        binding,
        rng,
    )
    .map_err(|_| PairingError::Adapter)
}

/// Completes SPAKE2 Party A with the responder's share.
pub(crate) fn finish_initiator(
    state: PartyAState<P256Suite>,
    peer_share: &[u8],
) -> Result<Spake2Output, PairingError> {
    state
        .finish(peer_share)
        .map_err(|_| PairingError::Authentication)
}

/// Starts SPAKE2 Party B (the pairing responder).
pub(crate) fn start_responder(
    code: &PairingCode,
    binding: &[u8],
    rng: &mut impl CryptoRng,
) -> Result<(Vec<u8>, PartyBState<P256Suite>), PairingError> {
    let password = code.password_scalar()?;
    PartyB::<P256Suite>::start(
        &password,
        IDENTITY_INITIATOR,
        IDENTITY_RESPONDER,
        binding,
        rng,
    )
    .map_err(|_| PairingError::Adapter)
}

/// Completes SPAKE2 Party B with the initiator's share.
pub(crate) fn finish_responder(
    state: PartyBState<P256Suite>,
    peer_share: &[u8],
) -> Result<Spake2Output, PairingError> {
    state
        .finish(peer_share)
        .map_err(|_| PairingError::Authentication)
}

/// Returns this party's confirmation tag, which is safe to send.
pub(crate) fn confirmation(output: &Spake2Output) -> Result<[u8; CONFIRM_BYTES], PairingError> {
    output
        .confirmation_mac
        .as_slice()
        .try_into()
        .map_err(|_| PairingError::Adapter)
}

/// Verifies the peer's confirmation tag in constant time.
pub(crate) fn verify(output: &Spake2Output, peer_tag: &[u8]) -> Result<(), PairingError> {
    output
        .verify_peer_confirmation(peer_tag)
        .map_err(|_| PairingError::Authentication)
}

/// Appends one length-prefixed binding field.
fn push_field(binding: &mut Vec<u8>, field: &[u8]) {
    binding.extend_from_slice(&(field.len() as u32).to_be_bytes());
    binding.extend_from_slice(field);
}

#[cfg(test)]
mod tests {
    use pakery_spake2::{PartyA, PartyB};
    use rand_core::{OsRng, UnwrapErr};

    use super::{
        P256Suite, PairingCode, PairingError, binding, confirmation, finish_initiator,
        finish_responder, start_initiator, start_responder,
    };

    /// Rejects a code used by the tests so a random collision cannot flake.
    fn another_code(code: &PairingCode) -> PairingCode {
        let mut value = 0u32;
        loop {
            let candidate = PairingCode::parse(&format!("{value:08}")).unwrap();
            if &candidate != code {
                return candidate;
            }
            value += 1;
        }
    }

    #[test]
    fn codes_are_eight_digits_with_leading_zeroes_and_redacted_debug() {
        let code = PairingCode::generate(&mut UnwrapErr(OsRng));
        let grouped = code.grouped();
        assert_eq!(grouped.replace(' ', "").len(), 8);
        assert!(
            grouped
                .bytes()
                .all(|byte| byte == b' ' || byte.is_ascii_digit())
        );

        let leading = PairingCode::parse("00000000").unwrap();
        assert_eq!(leading.grouped(), "0000 0000");
        assert_eq!(
            PairingCode::parse("99999999").unwrap().grouped(),
            "9999 9999"
        );
        assert_eq!(format!("{leading:?}"), "PairingCode([REDACTED])");
        assert!(!format!("{leading:?}").contains('0'));

        for invalid in [
            "",
            "1234567",
            "123456789",
            "1234 567",
            "1234a678",
            "⁻¹²³⁴⁵⁶⁷⁸",
        ] {
            assert!(PairingCode::parse(invalid).is_none(), "input: {invalid:?}");
        }
    }

    #[test]
    fn binding_requires_the_fixed_exporter_length_and_hello_bounds() {
        assert!(binding(&[0u8; 32], b"a", b"b").is_ok());
        assert_eq!(
            binding(&[0u8; 31], b"a", b"b"),
            Err(PairingError::InvalidInput)
        );
        let oversized = vec![b'x'; super::MAX_HELLO_BODY_BYTES + 1];
        assert_eq!(
            binding(&[0u8; 32], &oversized, b"b"),
            Err(PairingError::InvalidInput)
        );
    }

    #[test]
    fn matching_codes_confirm_and_mismatched_codes_or_bindings_fail() {
        let mut rng = UnwrapErr(OsRng);
        let code = PairingCode::generate(&mut rng);
        let aad = binding(&[7u8; 32], br#"{"type":"hello","version":1}"#, b"{}").unwrap();

        let (share_a, state_a) = start_initiator(&code, &aad, &mut rng).unwrap();
        let (share_b, state_b) = start_responder(&code, &aad, &mut rng).unwrap();
        let output_a = finish_initiator(state_a, &share_b).unwrap();
        let output_b = finish_responder(state_b, &share_a).unwrap();

        let tag_a = confirmation(&output_a).unwrap();
        let tag_b = confirmation(&output_b).unwrap();
        super::verify(&output_a, &tag_b).unwrap();
        super::verify(&output_b, &tag_a).unwrap();

        // A different code derives a different key schedule even though the
        // share exchange itself succeeds; the confirmation must reject it.
        let wrong = another_code(&code);
        let (_, wrong_state) = start_responder(&wrong, &aad, &mut rng).unwrap();
        let wrong_output = finish_responder(wrong_state, &share_a).unwrap();
        assert_eq!(
            super::verify(&output_a, &wrong_output.confirmation_mac),
            Err(PairingError::Authentication)
        );

        // A changed hello binding must fail confirmation as well.
        let rebound = binding(&[7u8; 32], b"{}", br#"{"type":"hello","version":1}"#).unwrap();
        let (_, other_state) = start_responder(&code, &rebound, &mut rng).unwrap();
        let other_output = finish_responder(other_state, &share_a).unwrap();
        assert_eq!(
            super::verify(&output_a, &other_output.confirmation_mac),
            Err(PairingError::Authentication)
        );
    }

    #[test]
    fn rfc9382_p256_vector_matches_exactly() {
        let w = scalar("2ee57912099d31560b3a44b1184b9b4866e904c49d12ac5042c97dca461b1a5f");
        let x = scalar("43dd0fd7215bdcb482879fca3220c6a968e66d70b1356cac18bb26c84a78d729");
        let y = scalar("dcb60106f276b02606d8ef0a328c02e4b629f84f89786af5befb0bc75b6e66be");

        let (share_a, state_a) =
            PartyA::<P256Suite>::start_with_scalar(&w, &x, b"server", b"client", b"").unwrap();
        assert_eq!(
            share_a,
            hex(
                "04a56fa807caaa53a4d28dbb9853b9815c61a411118a6fe516a8798434751470f9010153ac33d0d5f2047ffdb1a3e42c9b4e6be662766e1eeb4116988ede5f912c"
            )
        );

        let (share_b, state_b) =
            PartyB::<P256Suite>::start_with_scalar(&w, &y, b"server", b"client", b"").unwrap();
        assert_eq!(
            share_b,
            hex(
                "0406557e482bd03097ad0cbaa5df82115460d951e3451962f1eaf4367a420676d09857ccbc522686c83d1852abfa8ed6e4a1155cf8f1543ceca528afb591a1e0b7"
            )
        );

        let output_a = state_a.finish(&share_b).unwrap();
        let output_b = state_b.finish(&share_a).unwrap();
        assert_eq!(
            output_a.session_key.as_bytes(),
            hex("0e0672dc86f8e45565d338b0540abe69")
        );
        assert_eq!(
            output_a.confirmation_mac,
            hex("58ad4aa88e0b60d5061eb6b5dd93e80d9c4f00d127c65b3b35b1b5281fee38f0")
        );
        assert_eq!(
            output_b.confirmation_mac,
            hex("d3e2e547f1ae04f2dbdbf0fc4b79f8ecff2dff314b5d32fe9fcef2fb26dc459b")
        );
        output_a
            .verify_peer_confirmation(&output_b.confirmation_mac)
            .unwrap();
        output_b
            .verify_peer_confirmation(&output_a.confirmation_mac)
            .unwrap();
    }

    /// Converts a 32-byte big-endian scalar into the group's wide form.
    fn scalar(
        encoded: &str,
    ) -> <pakery_crypto::P256Group as pakery_core::crypto::CpaceGroup>::Scalar {
        use pakery_core::crypto::CpaceGroup;

        let bytes = hex(encoded);
        let mut wide = [0u8; 64];
        wide[64 - bytes.len()..].copy_from_slice(&bytes);
        pakery_crypto::P256Group::scalar_from_wide_bytes(&wide).unwrap()
    }

    /// Decodes lowercase hex in tests.
    fn hex(encoded: &str) -> Vec<u8> {
        let (pairs, _) = encoded.as_bytes().as_chunks::<2>();
        pairs
            .iter()
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect()
    }
}
