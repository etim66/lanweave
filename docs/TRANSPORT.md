# Transport

## Profile

Protocol version 1 uses one TCP connection for one authorized session. The session may carry several sequential transfer exchanges in either direction.

TLS 1.3 starts immediately after TCP connection. ALPN is exactly `lanweave/1`. TLS 1.2, resumption, tickets, pre-shared keys, 0-RTT, and early application data are disabled.

The responder creates a fresh in-memory self-signed P-256 certificate and private key for each connection. This certificate encrypts the provisional connection but is not a trusted identity. Pairing later confirms the intended live connection.

The initiator's custom certificate verifier may relax only public trust-chain and server-name checks. It must still validate the required certificate shape, proof of private-key possession, and TLS 1.3 handshake signatures. This custom verifier requires specialist review.

## Pairing Binding

After TLS, the peers exchange exact `hello` bodies, then complete `pair_request` and `pair_response`. On acceptance, the responder displays a locally generated code and the initiator enters it.

The fixed pairing profile binds its mutual confirmation to:

- the current TLS exporter;
- the exact initiator `hello` JSON body;
- the exact responder `hello` JSON body; and
- fixed protocol labels and pairing roles.

The code is not sent on the connection. Pairing output is used for confirmation, not as a second file-encryption key. TLS remains the only record-protection layer.

## Rust Mapping

The implementation uses the `ring` crypto provider with SSL-over-1.3-only configuration:

- `rustls` 0.23 selected with the `std` and `ring` features (the `tls12` feature is off, so TLS 1.2 is unsupported at compile time);
- `tokio-rustls` 0.26 with the `ring` feature;
- `rcgen` 0.14 for the fresh P-256 certificate per connection; and
- `zeroize` for best-effort cleanup of the exporter.

Release builds must disable key logging. The pairing dependency is a prototype around `pakery-spake2` and
`pakery-crypto` pending independent review; see [Cryptography](CRYPTOGRAPHY.md).

## Stream And Framing

TLS carries one reliable ordered stream. JSON controls and binary `DATA` use the fixed frame header in [Message Format](MESSAGE_FORMAT.md).

- Parsers support partial headers, partial bodies, and several frames in one socket read.
- Lengths are validated before allocation.
- Queues and file reads are bounded.
- One writer sends complete frames so control and data bytes cannot interleave.
- A terminal control may skip queued data only after unsent nonterminal frames are removed.

## Connection Lifecycle

The successful lifecycle is:

1. TCP and TLS 1.3.
2. Initiator `hello`, then responder `hello`.
3. `pair_request`, responder decision, and `pair_response`.
4. On acceptance, local code display and entry.
5. Four pairing records and mutual confirmation.
6. Zero or more sequential transfer proposals and transfers.
7. Manual `session_close`, 600-second idle expiry, app shutdown, or connection loss.

A transfer rejection does not close the connection. A successful final `file_result` completes only that transfer. Both peers return to session idle and may change transfer direction.

## Timeouts

Use monotonic clocks for:

- connection and TLS setup;
- pairing request decisions;
- the fixed 120-second code lifetime;
- transfer decisions;
- data progress and file results; and
- the fixed 600-second session idle maximum.

An active transfer pauses the session idle timer but must continue to make progress within its own bounded deadline. An expired transfer-decision prompt sends a rejecting `transfer_response` with `timeout`. An expired data-progress or file-result deadline closes the session after cleanup.

## Closing

`session_close` gives the peer a clear reason when the connection is still writable. It is valid in any authorized session state and does not need an acknowledgement. If a transfer is active, both peers stop it and clean the current partial file. EOF, TLS close, and TCP close also end the session but never prove that an active file succeeded.

Closing drops all pairing and authorization state. TLS resumption cannot restore it. A new connection repeats the complete pairing flow.

## Deferred

QUIC, multiple streams, multiple simultaneous transfers, application AEAD, algorithm negotiation, session resume, persistent identities, and trusted devices are deferred.
