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

Discovery records do not contain pairing codes, identity fingerprints, file metadata, transfer state, capabilities, or trusted-device data. Service and host names are untrusted text and must be safely escaped before they reach a terminal.

## Device List

The TUI combines records for the same service instance and shows a bounded list of current candidates. It removes stale and goodbye records. Selecting a device starts a new connection and sends a pairing request; it does not mark that device as trusted.

Implementations must safely handle duplicate records, name conflicts, multiple interfaces, IPv4, scoped IPv6, changing addresses, and blocked multicast. Record counts, text lengths, retained candidates, resolution work, and connection attempts are bounded.

## Direct Address

A user may enter a host or IP address and port when multicast discovery is unavailable. Direct addressing skips only mDNS. It still requires the same pairing request, acceptance, one-time code, authorization, and transfer approval flow.

Discovered and direct routes feed the same application connection effect. The direct-address input and complete validation UI are added with device selection rather than creating a separate transport path.

## Security Limits

Any LAN device can publish a false Lanweave record, copy a display name, hide a real device with noise, or direct a connection to the wrong endpoint. Pairing is what confirms the intended live connection. Other LAN devices can also observe that Lanweave is running and see connection timing and traffic volume.
