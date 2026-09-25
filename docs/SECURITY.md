# Security

> **Experimental:** Lanweave has not been implemented or audited. It must not be described as secure or production-ready.

## Trust Boundary

The LAN, discovery records, display names, addresses, provisional peer, and all network input are untrusted. TLS encrypts the connection before pairing, but the intended peer is not confirmed until code authorization succeeds.

The pairing initiator selects a candidate device. The responder accepts or rejects the live request. After acceptance, the responder creates a local one-time code and shows it to the initiator through a private human channel. The code is never advertised or sent as a normal protocol field.

Pairing confirms the current TLS connection. It does not approve files. Every transfer recipient reviews the complete manifest and makes a separate decision.

## No Persistent Trust

Authorization belongs only to one live connection. Lanweave must not store:

- trusted-device records;
- reusable pairing codes or secrets;
- persistent identity keys for reconnect authorization;
- TLS sessions or tickets; or
- automatic reconnect permission.

Manual close, idle close, app exit, or connection loss removes authorization. A future connection repeats the full request, accept, code, and confirmation flow.

## Session Limits

Only one proposal or transfer is active at a time. An expired transfer prompt rejects the proposal. An expired data-progress or file-result deadline closes the session. A session with no proposal or active transfer closes after 600 consecutive seconds.

Pairing requests, transfer requests, cryptographic work, open files, storage, queues, and TUI prompts all have global and per-source limits. This reduces resource abuse but cannot guarantee availability.

## Input And Terminal Safety

Control messages use strict bounded JSON. Parsers reject duplicate fields, unknown fields, invalid UTF-8, unsupported numbers, excessive nesting, and oversized frames before expensive work.

Peer names and filenames are data, not terminal markup. The TUI and logs escape terminal control sequences, control characters, and bidirectional text controls. Remote errors never include local paths, stack traces, parser positions, or operating-system messages.

## File Consent And Safety

The requester sends no filename or size before pairing. The recipient sees the complete file list before acceptance. Pairing is never automatic transfer approval.

Incoming names are single filename components under a local destination. Lanweave rejects unsafe, duplicate, platform-equivalent, and existing names. It writes to restrictive temporary files and finalizes only after exact size and SHA-256 verification. It never overwrites or automatically opens a received file.

## Secret Handling

Pairing codes, password-derived values, ephemeral pairing values, exporters, confirmation tags, and TLS private keys must not appear in logs or crash reports. Implementations use best-effort zeroization, while clearly stating that complete physical erasure cannot be guaranteed.

The fixed TLS and SPAKE2 composition, certificate verifier, password mapping, dependency tree, and implementation require specialist review and deterministic positive and negative test vectors before a security claim.

## Remaining Risks

- A person who sees the live code may pair during its lifetime.
- A malicious paired peer may send harmful file content.
- Discovery can be spoofed or flooded.
- LAN observers can see endpoints, timing, and traffic volume.
- A compromised endpoint can read files and secrets.
- Denial of service cannot be fully prevented.
- `/update` trusts GitHub Releases and release checksums for new binaries, and
  Windows builds are not code-signed yet. Eligible copies also contact GitHub
  once at startup for the silent release check. See [Updates](UPDATES.md) for the
  eligibility rules and the trust model.
