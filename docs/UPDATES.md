<!-- SPDX-FileCopyrightText: 2026 Unyime Etim -->
<!-- SPDX-License-Identifier: Apache-2.0 -->

# Updates

`/update` checks GitHub Releases for a newer stable version and, after an
explicit confirmation, installs it in place. The new version runs the next time
Lanweave starts.

On startup, an eligible copy checks once in the background. If a newer stable
version exists, the home and device-list screens show
`/update to install vX.Y.Z`; nothing is downloaded by that check. A failed
startup check is silent and changes nothing.

## The flow

1. On startup, an eligible copy silently checks the latest release once and
   shows `/update to install vX.Y.Z` when a newer stable version exists. The
   notice stays until that version is installed or the running version becomes
   current.
2. `/update` opens a dialog and checks the latest release on GitHub.
3. If the running version is current, the dialog says so and closes.
4. If a newer version exists, the dialog shows both versions and asks for
   confirmation. **Cancel** or Escape changes nothing.
5. **Update** downloads the release installer and runs it against the original
   install directory. The terminal UI shows an installing note until it
   finishes.
6. On success the dialog offers **Quit now** or **Later**. Quitting follows the
   normal shutdown path and restores the terminal; the next launch uses the new
   version.

The check and the install use GitHub only. There is no Lanweave update server:
the only automatic request is the single startup check, and an ineligible copy
never contacts GitHub at all. The startup check never downloads or changes
anything; offline, rate-limited, and other failures are silent.

## Eligibility

Only copies installed by the release installer can update themselves. The
installer writes an install receipt under `~/.config/lanweave` on Linux and
`%LOCALAPPDATA%\lanweave` on Windows that records the installed version and
directory; `/update` reads it and refuses to replace a running executable that
does not match it.

The following copies are **not** eligible and show an install hint instead:

- development builds (`cargo run`, `cargo build`);
- `cargo install` copies;
- copies from a distro package manager; and
- any copy whose install receipt was removed.

Ineligible copies make no startup request and show no notice; running `/update`
in them explains how to install the latest release manually.

This is deliberate: replacing a binary that another tool installed would leave
that tool's metadata describing a version that is no longer on disk.

## Trust model

- Releases are fetched over HTTPS from `github.com` and the GitHub API.
- The installed version is verified against the release checksums before the
  installer runs, and the installer is forced to the recorded install
  directory.
- The updater never executes release-provided scripts other than the release's
  own installer, and it accepts no update instructions from network data.
- Release artifacts are built by GitHub Actions from tagged commits. Optional
  GitHub attestations can be verified with `gh attestation verify` after
  downloading an archive manually.
- Windows binaries are not code-signed yet, so SmartScreen may warn on the
  first manual download. The installer path does not require elevation.

GitHub rate-limits unauthenticated release checks by IP address. If a check
fails with a network message on a shared connection, set `LANWEAVE_GITHUB_TOKEN`
to a token before starting Lanweave; it is used for the GitHub API request and
is never written to disk.

## Failure behavior

- A failed `/update` check reports a short reason and changes nothing; the
  startup check stays silent and leaves any earlier notice in place.
- A failed install leaves the current copy untouched. The installer writes the
  new copy before replacing the old one, and Windows keeps a
  `lanweave.exe.previous.exe` copy only long enough to delete it on the next
  launch.
- If the dialog is closed by a required prompt while a check or install is
  running, the result is dropped; running `/update` again restarts the flow.

## Uninstalling

Linux:

```sh
rm -rf ~/.lanweave
# Remove the "~/.lanweave/bin" PATH line added by the installer, if present.
```

Windows (PowerShell):

```powershell
Remove-Item -Recurse -Force "$env:USERPROFILE\.lanweave"
# Then remove "%USERPROFILE%\.lanweave\bin" from the user PATH.
```

The install receipts under `~/.config/lanweave` or `%LOCALAPPDATA%\lanweave` can
be removed with the install directory.
