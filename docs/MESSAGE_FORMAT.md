# Message Format

This document defines experimental protocol version 1. All control messages use strict JSON. File bytes use binary `DATA` frames.

## JSON Rules

- A control body is one UTF-8 JSON object with no trailing value.
- Duplicate and unknown fields are rejected.
- Numbers are integers from `0` through `2^53-1`. Floats and exponent notation are rejected.
- `null` is not used.
- Field names, arrays, strings, nesting, and total body size have fixed bounds.
- Binary pairing values use canonical unpadded base64url.
- SHA-256 values are 64 lowercase hexadecimal characters.
- The generic JSON body limit is 1 MiB. `hello` is at most 4,000 bytes, `pairing` is at most 4,096 bytes, and `transfer_request` is at most 256 KiB.

## Messages

### `hello`

```json
{"type":"hello","version":1,"display_name":"Workstation"}
```

`version` is exactly `1`. `display_name` is optional, untrusted display text limited to 128 UTF-8 bytes. The initiator sends the first `hello` and the responder sends the second. Both peers retain the exact body bytes through pairing.

### `pair_request`

```json
{"type":"pair_request"}
```

The initiator sends this after both `hello` messages. It asks the responder to show a pairing prompt. It does not contain a code, identity claim, or file metadata.

### `pair_response`

Accepted:

```json
{"type":"pair_response","accepted":true}
```

Rejected:

```json
{"type":"pair_response","accepted":false,"reason":"user_rejected"}
```

`reason` is present only for rejection. Allowed reasons are `user_rejected`, `busy`, `timeout`, and `unavailable`. Rejection closes the connection. After acceptance, the responder creates and displays the code locally.

### `pairing`

Share:

```json
{"type":"pairing","step":"share","data":"BGsX0fLhLEJH-Lzm5WOkQPJ3A32BLeszoPShOUXYmMKWT-NC4v4af5uO5-tKfA-eFivOM1drMV7Oy7ZAaDe_UfU"}
```

Confirmation:

```json
{"type":"pairing","step":"confirm","data":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"}
```

Exactly four records occur after an accepted `pair_response`: initiator Party A share, responder Party B share, initiator confirmation, and responder confirmation. Direction and state determine the role. The code never appears in this message.

### `transfer_request`

```json
{"type":"transfer_request","files":[{"name":"report.pdf","size":1048576},{"name":"empty.txt","size":0}]}
```

```json
{"type":"transfer_request","files":[{"name":"docs.zip","size":1234,"kind":"folder","items":42,"source_size":987654}]}
```

`files` contains 1 through 1,024 entries. Each entry contains `name` and `size`. `kind`, `items`, and `source_size` are optional folder metadata: `kind` is `folder`, `items` counts the files and directories inside the folder, and `source_size` is the uncompressed regular-file total. The fields are required together for a folder archive and forbidden on a regular file; an absent `kind` means a regular file. `name` is a filename component, not a path. Array order is transfer order. The request is immutable after it is sent.

### `transfer_response`

```json
{"type":"transfer_response","accepted":true}
```

```json
{"type":"transfer_response","accepted":false,"reason":"user_rejected"}
```

Allowed rejection reasons are `user_rejected`, `busy`, `timeout`, `invalid_manifest`, `invalid_filename`, `name_conflict`, `destination_exists`, `insufficient_storage`, `resource_limit`, and `unavailable`. Rejection ends the proposal and returns the session to idle.

### `ready`

```json
{"type":"ready"}
```

The recipient sends `ready` immediately after accepting and preparing the destination. The requester MUST wait for it before sending `DATA`.

### `file_end`

```json
{"type":"file_end","index":0,"sha256":"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"}
```

`index` is the current zero-based manifest index. `sha256` covers exactly the file bytes sent.

### `file_result`

```json
{"type":"file_result","index":0,"status":"verified"}
```

```json
{"type":"file_result","index":0,"status":"failed","code":"hash_mismatch"}
```

Allowed failure codes are `size_mismatch`, `hash_mismatch`, `write_failed`, `destination_exists`, and `insufficient_storage`. A failed result ends the current transfer, cleans the partial file, and closes the session so in-flight file frames cannot be mistaken for a later transfer.

### `transfer_cancel`

```json
{"type":"transfer_cancel","code":"user_cancelled"}
```

Allowed codes are `user_cancelled` and `source_unavailable`. Before `ready`, cancellation ends the proposal and returns to idle. After `ready`, it stops the transfer, deletes the current partial file, and closes the session.

### `session_close`

```json
{"type":"session_close","code":"user_closed"}
```

Allowed codes are `user_closed`, `idle_timeout`, and `shutdown`. This message is valid in any authorized session state, is terminal, and is not acknowledged.

### `error`

```json
{"type":"error","code":"invalid_message"}
```

Allowed codes are `unsupported_version`, `invalid_message`, `authentication_failed`, `timeout`, `resource_limit`, and `internal_error`. `error` is terminal. A peer never answers an `error` with another `error`.

## Correlation

Version 1 has no request, transfer, session, or file IDs. One ordered connection, one active proposal or transfer, manifest index, and the simultaneous-request priority rule provide context. When the responder yields a simultaneous local proposal, it tracks and consumes exactly one stale `transfer_response` with reason `busy` for that withdrawn proposal.

## Framing

Every JSON control and binary chunk uses this 12-byte header:

```text
0       4     magic: ASCII "LNWV"
4       1     frame version: 1
5       1     kind: 0 = JSON, 1 = DATA
6       2     header length: unsigned big-endian 12
8       4     body length: unsigned big-endian
12      N     body
```

A parser validates the complete header and body length before allocation. It handles partial and combined socket reads. Invalid framing closes the connection.

`DATA` contains 1 through 1 MiB raw bytes for the current file. A zero-byte file has no `DATA` frame. `DATA` has no JSON, index, or offset because connection state supplies that context.
