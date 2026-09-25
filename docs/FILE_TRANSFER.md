# File Transfer

This document defines file handling for protocol version 1. See [Protocol](PROTOCOL.md) for the session flow and [Message Format](MESSAGE_FORMAT.md) for wire fields.

## Selecting Files In The TUI

A user can paste one or more local file or folder paths into the file area. The TUI parses pasted paths, removes exact duplicates, checks that each item is a readable regular file or folder, and shows a review list before **Send** is enabled. Pasted paths are reviewed immediately, so the next Enter sends from an authorized idle session.

Pasted text is split on newlines. Each entry is trimmed, `file://` URIs are percent-decoded, and one matching pair of outer single or double quotes is removed; inside double quotes only `\\` and `\"` are unescaped. A backslash-escaped path (as terminals produce for dragged files) is retried unescaped when the literal path does not exist. Paths are never passed to a shell, and bare lines are kept exactly as pasted so spaces and platform separators survive. Pasted paths are inspected off the event loop, so a large folder never freezes the terminal; the review shows a reviewing note until the inspection finishes. The review list can be prepared while browsing, but **Send** is enabled only in an authorized idle session.

A folder is reviewed as one entry named `<folder>.zip` and sent as a single zip archive. Review shows the item count and uncompressed size; the manifest carries the built zip's exact byte size plus untrusted `items` and `source_size` hints for the recipient. Symlinks and special files inside a folder are skipped and never followed. Folders are compressed entry by entry into a temporary file, with bounded item counts, and the temporary archive is deleted when the transfer ends. A local cancel stops compression between entries, before any request is sent. If a folder cannot be compressed (it changed, contains unsupported names, or exceeds the entry bound), the send is abandoned with a local notice instead of a silent rejection.

Local paths stay local. The peer receives only each base filename and exact byte size. Regular files, files-only selections, and folder archives are supported in version 1; symlinks and special files are rejected or skipped.

## Separate Approval

An authorized session is required before file metadata is sent. The requester sends one complete `transfer_request`. The recipient sees:

- each filename and size;
- file count;
- checked total size; and
- the requesting peer's untrusted display name.

The recipient accepts or rejects the whole list. There is no partial approval, remote rename, or overwrite option. A rejection returns both peers to session idle. Long manifests scroll with the arrow keys, PageUp/PageDown, and Home/End, so every entry can be reviewed before the decision.

## Manifest

A manifest contains 1 through 1,024 ordered entries:

- `name`: one non-empty filename component, at most 255 UTF-8 bytes;
- `size`: the exact byte length from zero through `2^53-1`; and
- optional folder metadata: `kind` is `folder`, with `items` (files and directories inside) and `source_size` (uncompressed regular-file total), required only for folder archives and forbidden on regular files.

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

The local user can cancel with Escape or `/cancel`. A cancel while a folder archive is being prepared stops the preparation between entries, abandons the proposal before any `transfer_request` is sent, and shows a cancelled summary. Before `ready`, cancellation ends the proposal: the cancelling side shows a cancelled summary and the peer returns to idle. After `ready`, it sends `transfer_cancel`, deletes the current partial file, closes the session, and shows a cancelled summary on both sides. Cancelled summaries list only the entries that were verified. A reported failure or cancellation after `ready` closes the session after cleanup because file data may already be in flight. Malformed framing, transport loss, or a terminal protocol error also closes the session.

Received files are never automatically opened, previewed, or executed.

## Progress

The TUI distinguishes:

- bytes read locally;
- bytes written to the transport;
- bytes received by the peer; and
- files confirmed as verified.

Per-file and overall progress bars, average speed, and estimated time left are derived from throttled progress events. Transport writes are not proof that a file was saved. `file_result` is the final file-level result.

The active transfer list follows the file in flight, so the current entry stays visible as files arrive. Scrolling with the arrow keys, PageUp/PageDown, or Home pauses the follow; End returns to the live view. The completed summary scrolls the same way, and the sender and recipient see the same controls.

## Deferred

Resume, sparse files, paths, extra metadata beyond folder hints, chunk acknowledgements, overwrite, rename, parallel files, and concurrent transfers are not part of version 1.
