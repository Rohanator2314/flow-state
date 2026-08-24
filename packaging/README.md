# Distribution packaging

## Arch Linux / AUR

The complete AUR source package lives in `packaging/aur`. It deliberately
contains the desktop launcher, AppStream metadata, and a 256 px icon so the
AUR repository can be published without mutable external helper files.

For a new release:

1. Update `pkgver` in `packaging/aur/PKGBUILD` and set `pkgrel=1`.
2. Update the AppStream release version and date.
3. Replace the source archive checksum and any changed helper-file checksums.
4. Run `makepkg --printsrcinfo > .SRCINFO` from `packaging/aur`.
5. Build with `makepkg -f` and inspect the package with `namcap`.
6. Copy the contents of `packaging/aur` to the separate AUR Git repository,
   commit them on its `master` branch, and push.

The package builds from the immutable GitHub release tag with Cargo's locked
dependency graph. The `check()` phase excludes the external-tool-dependent
LaTeX tests, matching release CI.

## Windows

`wix/main.wxs` is the cargo-wix definition used by the Windows release job.
Its `UpgradeCode` and the fixed GUID for PATH management are product identity;
do not regenerate them between versions. `wix/flow-state.ico` is generated
from `assets/flow-state-icon.png` and supplies the installer and Start Menu
icon.

The release workflow pins cargo-wix and uploads
`flow-state-windows-x64.msi`. It is currently unsigned; Authenticode signing
can be inserted before the upload once a Windows signing certificate is
available.
