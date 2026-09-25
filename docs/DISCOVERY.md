# Discovery

## Purpose

Lanweave uses mDNS/DNS-SD to show devices that are currently running the app on the local network. Discovery answers only: "Where might a Lanweave listener be?" It does not prove identity or authorize a connection.

A device advertises and accepts requests only while its Lanweave process is running. There is no background daemon, offline status, trusted-device cache, or persistent advertisement. After a crash or forced exit, another device may briefly show a cached stale record until mDNS expiry; connection will fail and the record must be removed.

## App Lifecycle

1. Start the TCP listener.
2. Start browsing for Lanweave services.
3. Advertise the listener.
4. Add, update, and remove devices in the TUI as records change or expire.
5. Stop advertising, browsing, and listening when the app exits.

The current listener binds an ephemeral port on the IPv4 wildcard address, so it accepts connections on every active IPv4 interface. IPv6 listener and endpoint preference will be selected with the device connection policy; Lanweave does not rely on platform-dependent dual-stack socket defaults.

Until pairing transport is implemented, the listener accepts and immediately closes inbound TCP connections without sending per-connection application events. It never treats these temporary connections as authorized pairing requests.

Discovery may disappear while a paired session is active. This does not close or change that session.

## Service Record

The proposed service type is:

```text
_lanweave._tcp.local.
```

SRV supplies the host and port. TXT contains only this untrusted hint:

| Key | Value | Meaning |
| --- | --- | --- |
| `v` | `1` | The listener expects experimental protocol version 1 |

The service instance is the local computer name, sanitized into a DNS label with its original casing; the host record stays a per-run lowercase label so it cannot collide with the platform mDNS responder. The `hello` display name carries the same computer name.

Discovery records do not contain pairing codes, identity fingerprints, file metadata, transfer state, capabilities, or trusted-device data. Service and host names are untrusted text and must be safely escaped before they reach a terminal.

## Device List

The TUI combines records for the same service instance and shows a bounded, sorted list of current candidates. Sorting is deterministic: case-insensitive display name, then a stable store-assigned id so equal names keep discovery order. It removes stale and goodbye records.

Selecting a device starts a new connection and sends a pairing request; it does not mark that device as trusted. Up/Down move the selection and Enter connects. The selection follows its device across updates and is cleared when that device disappears, so a removed device can never be connected through stale UI state. Device names and addresses are shown with an untrusted marker and are never presented as verified identity.

Implementations must safely handle duplicate records, name conflicts, multiple interfaces, IPv4, scoped IPv6, changing addresses, and blocked multicast. Record counts, text lengths, retained candidates, resolution work, and connection attempts are bounded.

## Direct Address

A user may enter a host or IP address and port when multicast discovery is unavailable. `/connect` opens the input; the address is validated live and the connection can only start after the input parses as a host (no whitespace or control bytes, at most 255 bytes) and a port (one to five decimal digits in 1..=65535). Direct addressing skips only mDNS. It still requires the same pairing request, acceptance, one-time code, authorization, and transfer approval flow.

Discovered and direct routes feed the same application connection effect. The direct-address input and complete validation UI are added with device selection rather than creating a separate transport path.

## Security Limits

Any LAN device can publish a false Lanweave record, copy a display name, hide a real device with noise, or direct a connection to the wrong endpoint. Pairing is what confirms the intended live connection. Other LAN devices can also observe that Lanweave is running and see connection timing and traffic volume.
