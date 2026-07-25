# Project Overview

## Status

Lanweave is currently a work in progress. The application is a Rust terminal user interface for local file transfer. Discovery, networking, security, file transfer, and the TUI are being implemented.

## Product Model

Lanweave runs as a long-lived TUI. Starting `lanweave` starts the terminal interface, the local listener, and network discovery. Exiting the app stops discovery and makes the device unavailable.

The product has three separate user decisions:

1. **Pairing decision:** the selected device accepts or rejects a pairing request.
2. **Code authorization:** the pairing responder shows a one-time code to the initiator, who enters it.
3. **Transfer decision:** the recipient reviews and accepts or rejects every proposed file list.

These decisions cannot approve one another. Accepting a pairing request does not authorize the session until the code succeeds. An authorized session does not approve any file transfer.

## Session Flow

1. Both users run Lanweave.
2. The initiator selects a visible device and sends a pairing request.
3. The responder accepts or rejects the request.
4. On acceptance, the responder's app creates and displays a one-time code.
5. The initiator enters the code. The peers use it to confirm that the encrypted connection reaches the intended devices.
6. The session becomes active. Either participant may propose a transfer.
7. The recipient reviews file metadata and accepts or rejects the complete request.
8. A successful transfer returns the session to idle. A failure after file data starts cleans the partial file and closes the session.
9. More transfers may be proposed in either direction.
10. A manual close, connection loss, app exit, or 10 minutes of session idle time ends the session.

Authorization exists only in memory for the current connection. A new session always requires a new pairing request and code. There are no trusted devices or automatic reconnects.

## Transfer Rules

A transfer request contains an ordered list of one or more regular files. Each item contains only its filename and exact size. The recipient also sees the file count and total size calculated by the app.

Files are sent one at a time. Lanweave writes incoming bytes to a temporary file, checks the size and SHA-256 digest, and then safely moves the file to its final name. It never overwrites an existing file.

The transfer stops on the first file failure. The current partial file is deleted and files already finalized remain. Rejection before `ready` keeps the paired session open. Failure after `ready` closes it because file frames may already be in flight.

Only one transfer request or active transfer may exist at a time. If both participants send a request at nearly the same time, the pairing initiator's request wins and the responder's request returns to its local queue. This keeps version 1 messages simple and avoids hidden concurrency.

## Idle Rules

The session idle timer starts when pairing completes and restarts whenever a transfer finishes or is rejected. It expires after 600 consecutive seconds without a new transfer request. An active transfer uses separate progress timeouts and does not count as idle.

Pairing prompts and transfer approval prompts also have shorter local deadlines so an unattended prompt cannot hold resources forever. The exact prompt deadlines are implementation policy; the session idle maximum is fixed at 10 minutes.

## Goals

- Make local file transfer easy from a terminal.
- Show only devices that are currently running Lanweave.
- Require clear approval for pairing and for each transfer.
- Keep all session authorization temporary.
- Allow either participant to send files during an active session.
- Use bounded memory, queues, parsing, and disk operations.
- Avoid a required account, cloud service, or internet connection.
- Keep the wire protocol small enough to test thoroughly.

## Non-Goals

The MVP excludes directories, resume, parallel files, compression, overwrite or rename negotiation, trusted devices, automatic acceptance, QUIC, mobile clients, and graphical interfaces. A TUI is required and is not considered a graphical interface.

Lanweave does not provide anonymity, protect a compromised computer, make received content safe to open, or guarantee connectivity on networks that block peer traffic.

## Success Criteria

An MVP candidate must:

- open a usable TUI on supported desktop terminals;
- list commands when the user enters `/`;
- start and stop discovery with the app;
- complete pairing request, accept/reject, code, and authorization flows;
- send multiple files safely after separate recipient approval;
- support later transfers in either direction in one session;
- close manually and after at most 10 idle minutes;
- require fresh authorization for every new session;
- pass cross-platform filesystem, protocol, fault, and security-profile tests; and
- complete the cryptographic review and dependency audit before claiming security.
