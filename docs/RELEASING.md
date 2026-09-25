<!-- SPDX-FileCopyrightText: 2026 Unyime Etim -->
<!-- SPDX-License-Identifier: Apache-2.0 -->

# Releasing

Lanweave releases are built, packaged, and published entirely by GitHub
Actions. The release pipeline is generated and maintained by
[cargo-dist](https://opensource.axo.dev/cargo-dist/) from
[`dist-workspace.toml`](../dist-workspace.toml), so the workflow and the local
configuration always agree. The dist version is pinned in that file and used
by `make dist-tools` and CI.

## Versioning

- The `version` field in `Cargo.toml` is the single source of truth for the
  application version.
- A release tag is `vX.Y.Z` (SemVer) and must match `Cargo.toml`. Pushing a
  mismatched tag fails the release before anything is published.
- Prereleases use `vX.Y.Z-rc.1` and are marked as prereleases on GitHub. They
  exist for testing the pipeline; `/update` only offers stable releases.
- Protocol and frame versions are separate from the application version; see
  [Versioning](VERSIONING.md).

## Cutting a release

1. Land the version bump in a pull request to `master`.
2. Tag the merge commit and push the tag:

   ```sh
   git checkout master
   git pull
   git tag v0.2.0
   git push origin v0.2.0
   ```

3. The `release` workflow runs plan, build, host, and announce:
   - builds `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl`, and
     `x86_64-pc-windows-msvc`;
   - writes per-artifact `.sha256` files, a `sha256.sum`, and
     `dist-manifest.json`;
   - creates the GitHub Release and uploads the archives, installers, and
     checksums; and
   - optionally attaches GitHub artifact attestations.

No release assets are built on a developer machine.

## Verifying a release

After the workflow succeeds:

1. On a clean Linux machine (glibc or musl), run the shell installer from the
   release body and confirm `lanweave --version` prints the new version.
2. On Windows, run the PowerShell installer and confirm the same.
3. If the previous release is still installed somewhere, run `/update` there
   and confirm the dialog offers the new version, installs it, and asks for a
   restart.
4. Optionally verify provenance for a downloaded archive:

   ```sh
   gh attestation verify lanweave-x86_64-unknown-linux-musl.tar.xz --repo etim66/lanweave
   ```

Install commands and `/update` behavior are documented in
[Updates](UPDATES.md).

## Local pipeline checks

`cargo dist plan` shows what a tag would produce, and
`cargo dist build --artifacts=local` builds and packages locally without
publishing. CI also runs `cargo dist generate --check`, so a configuration
change that would rewrite the release workflow is caught in review.

## Rollback

Published releases may already be installed, so prefer shipping a fixed patch
over deleting history. Delete the release and tag only when the pipeline itself
failed before any user could have installed the version; otherwise publish
`vX.Y.(Z+1)` with the fix and note it in the release body.

## Future packaging

The dist configuration can add installers without a new workflow: MSI for
Windows, Homebrew for macOS, and an npm package. Windows code signing and
attestation verification by `/update` are tracked as hardening work.
