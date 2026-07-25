# Lanweave

<!-- SPDX-FileCopyrightText: 2026 Unyime Etim -->
<!-- SPDX-License-Identifier: Apache-2.0 -->

Lanweave is a planned terminal user interface (TUI) for sending files to another device on the same local network. Run `lanweave` to open the app in the current terminal. Lanweave stays open while devices pair, review transfer requests, and send files in either direction.

Lanweave is currently a work in progress. Discovery, networking, security, file transfer, and the TUI are being implemented according to `IMPLEMENTATION_PLAN.txt` and the design in `docs/`.

## How It Works

1. Both users run `lanweave`. A device advertises and accepts requests only while Lanweave is running. A stale network record may remain visible briefly after an unclean exit, but connection will fail and the record will expire.
2. User 1 opens the device list, selects User 2's device, and requests pairing.
3. User 2 sees the request and accepts or rejects it.
4. If User 2 accepts, their Lanweave app creates and displays a one-time eight-digit code. User 2 shows that code to User 1.
5. User 1 enters the code. Lanweave checks the code and creates an authenticated, encrypted session between the two devices.
6. Either user can paste file paths into the TUI, review the files, and select **Send**.
7. The other user sees a request with the file names, sizes, count, and total size. They can accept or reject it.
8. Accepted files are sent in order. Each file is checked before it is saved under its final name.
9. After a transfer, either user can request another transfer in the same session.
10. Either user can close the session. Lanweave also closes it after 10 minutes with no transfer request or active transfer.

Closing the session removes its temporary authorization. The users must repeat the pairing and code flow before sending more files. Lanweave does not keep a trusted-device list.

## Terminal Interface

The app is interactive rather than a set of one-shot shell commands.

- Run `lanweave` to open the TUI.
- Enter `/` to see the commands available in the current screen.
- Use `/devices` to open the list of devices currently running Lanweave.
- Paste one or more file paths into the file area, review them, and select **Send**.
- Use `/disconnect` to close the current session and `/quit` to close Lanweave.

The exact command names may change during implementation, but `/` will always show the available actions.

## MVP Scope

The first release will support:

- Linux, macOS, and Windows desktop terminals;
- discovery on the local network while the app is running;
- one active paired session per app;
- one active transfer request at a time;
- one or more regular files in each request;
- transfer requests in either direction during a session;
- separate approval for pairing and for every transfer;
- encrypted TCP/TLS transport with code-based peer authorization;
- safe filenames, no overwrite, bounded memory use, and partial-file cleanup;
- manual session close and a maximum 10-minute idle period; and
- no accounts, cloud service, background daemon, or trusted devices.

Directories, resume, parallel file transfer, compression, overwrite or rename negotiation, QUIC, mobile clients, and graphical interfaces are outside the MVP.

## Technical Shape

The first implementation will be one Rust binary crate. Suggested modules are `tui`, `app`, `session`, `protocol`, `framing`, `transport`, `pairing`, `transfer`, `storage`, and `discovery`.

Likely dependency families include:

- `ratatui` and `crossterm` for the terminal interface;
- `tokio` for asynchronous tasks;
- `rustls` and `tokio-rustls` for TLS;
- `rcgen` for a fresh certificate per connection;
- `mdns-sd` for local discovery;
- `serde` and `serde_json` for control messages;
- `sha2` for file checks; and
- `zeroize` for best-effort secret cleanup.

The pairing library is not yet selected. Lanweave must not implement cryptographic group arithmetic itself. See [Cryptography](docs/CRYPTOGRAPHY.md) for the release-blocking security work.

## Building

Lanweave requires a stable Rust toolchain at or above MSRV 1.85. Common development commands:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all
cargo run
cargo audit
cargo deny check
```

The CI workflow at `.github/workflows/ci.yml` runs the same checks on Linux, macOS, and Windows.

## Documentation

| Document | Purpose |
| --- | --- |
| [Project overview](docs/PROJECT_OVERVIEW.md) | Product behavior, scope, and success criteria |
| [Protocol](docs/PROTOCOL.md) | Required connection, pairing, session, and transfer behavior |
| [Message format](docs/MESSAGE_FORMAT.md) | JSON controls, binary file frames, and limits |
| [State machines](docs/STATE_MACHINES.md) | TUI, pairing, session, and transfer states |
| [Sequence diagrams](docs/SEQUENCE_DIAGRAMS.md) | Main user and network flows |
| [Architecture](docs/ARCHITECTURE.md) | Module boundaries, events, and task ownership |
| [Discovery](docs/DISCOVERY.md) | Live device discovery and its trust limits |
| [File transfer](docs/FILE_TRANSFER.md) | File selection, approval, streaming, and cleanup |
| [Transport](docs/TRANSPORT.md) | TCP/TLS transport and session lifetime |
| [Cryptography](docs/CRYPTOGRAPHY.md) | Pairing profile and security review gates |
| [Security](docs/SECURITY.md) | Security requirements and boundaries |
| [Threat model](docs/THREAT_MODEL.md) | Threats, controls, and remaining risks |
| [Testing strategy](docs/TESTING_STRATEGY.md) | TUI, protocol, filesystem, and security tests |
| [Implementation roadmap](docs/IMPLEMENTATION_ROADMAP.md) | High-level dependency gates |
| [Glossary](docs/GLOSSARY.md) | Plain-language project terms |

## Security Status

Lanweave has not been implemented or audited. It is not safe for sensitive files. The planned pairing and TLS design still needs specialist review, test vectors, dependency review, and an audited RFC-conformant pairing implementation.

## Contributing And Licence

Design feedback is welcome, especially when it identifies a broken security or state rule and includes a reproducible example.

Lanweave is licensed under the Apache License, Version 2.0. See [`LICENCE`](LICENCE) for the full text and [`NOTICE`](NOTICE) for attribution.
