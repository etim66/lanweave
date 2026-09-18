# File Transfer

This document defines file handling for protocol version 1. See [Protocol](PROTOCOL.md) for the session flow and [Message Format](MESSAGE_FORMAT.md) for wire fields.

## Selecting Files In The TUI

A user can paste one or more local file paths into the file area. The TUI parses pasted paths, removes exact duplicates, checks that each item is a readable regular file, and shows a review list before **Send** is enabled.

Pasted text is split on newlines. Each entry is trimmed, and one matching pair of outer single or double quotes is removed; inside double quotes only `\\` and `\"` are unescaped. Paths are never passed to a shell, and bare lines are kept exactly as pasted so spaces and platform separators survive. The review list can be prepared while browsing, but **Send** is enabled only in an authorized idle session.

Local paths stay local. The peer receives only each base filename and exact byte size. Directories, symlinks, and special files are rejected in version 1.

## Separate Approval

An authorized session is required before file metadata is sent. The requester sends one complete `transfer_request`. The recipient sees:

- each filename and size;
- file count;
- checked total size; and
- the requesting peer's untrusted display name.

The recipient accepts or rejects the whole list. There is no partial approval, remote rename, or overwrite option. A rejection returns both peers to session idle.

## Manifest

A manifest contains 1 through 1,024 ordered entries:

- `name`: one non-empty filename component, at most 255 UTF-8 bytes;
- `size`: the exact byte length from zero through `2^53-1`.

Array position is the file index and transfer order. Count and total size are calculated with checked arithmetic. The manifest has no local paths, timestamps, permissions, media types, file IDs, or pre-transfer hashes.

The requester checks every source before proposing and again before sending it. If a source no longer matches the approved name, type, or size, the requester sends `transfer_cancel` rather than changing the manifest.

## Name And Destination Checks

Before acceptance, the recipient rejects the request if any name:

- is empty, dot-like, absolute, or path-like;
- contains a separator, NUL, or forbidden control character;
- is invalid or reserved on the destination platform;
- is equal or platform-equivalent to another requested name; or
- matches an existing destination entry.

After user approval and before sending acceptance, the recipient selects the destination and prepares a temporary file for the first item. The review prompt lets the recipient choose any existing directory; it is prefilled with the directory where Lanweave was started or the last destination they accepted. The peer never influences or learns the destination. Temporary names are unpredictable, created without following links, and use restrictive permissions. Failure during preparation sends a rejection.

Lanweave never overwrites or silently renames a destination. Finalization uses a safe no-replace operation. If that cannot be guaranteed, the file fails.

## Sending Data

After an accepting `transfer_response`, the recipient sends `ready`. The requester then sends each file in manifest order:

1. Send bounded raw `DATA` frames until exactly the declared size is sent.
2. Send `file_end(index, sha256)`.
3. Wait for `file_result`.
4. Start the next file only after a verified result.

The recipient writes and hashes accepted bytes as they arrive. A zero-byte file has no `DATA` frame and goes directly to `file_end`.

TCP/TLS provides reliable ordered bytes and backpressure. Lanweave still bounds socket, file, event, and writer queues so it never needs to hold a complete large file in memory.

## Verification

The recipient checks the exact byte count and SHA-256 digest before finalizing. A verified result is sent only after the file has its final no-overwrite name.

After the final verified result, the transfer is complete and the session returns to idle. Either participant may then propose another transfer.

## Failure And Cancellation

An early `file_end`, excess data, digest mismatch, write error, storage error, source change, or finalization error fails the current transfer. The recipient:

1. stops accepting data for the transfer;
2. deletes the current temporary file;
3. keeps files already verified in this transfer;
4. does not start later manifest entries; and
5. reports the failure when the connection is still safe.

A rejection or cancellation before `ready` returns the session to idle. A reported failure or cancellation after `ready` closes the session after cleanup because file data may already be in flight. Malformed framing, transport loss, or a terminal protocol error also closes the session.

Received files are never automatically opened, previewed, or executed.

## Progress

The TUI distinguishes:

- bytes read locally;
- bytes written to the transport;
- bytes received by the peer; and
- files confirmed as verified.

Transport writes are not proof that a file was saved. `file_result` is the final file-level result.

## Deferred

Resume, sparse files, directories, paths, extra metadata, chunk acknowledgements, overwrite, rename, parallel files, and concurrent transfers are not part of version 1.
