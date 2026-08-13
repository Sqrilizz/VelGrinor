# Package Manager Manifests

This directory contains manifest templates for various package managers. They are intentionally not directly installable while a checksum or commit is marked as a placeholder. The `Store publish` workflow resolves the release tag, validates SHA256 values from `SHA256SUMS.txt`, replaces every placeholder, and publishes the generated manifests.

## Status

| Package Manager | Type | Status | Installation Command |
|-----------------|------|--------|---------------------|
| **Homebrew** | CLI + Desktop | Ready | `brew tap Sqrilizz/velgrinor && brew install velgrinor` |
| **Winget** | Desktop | Template | `winget install Th0rgal.VelGrinorLauncher` |
| **Scoop** | CLI | Template | `scoop bucket add velgrinor https://github.com/Sqrilizz/scoop-velgrinor && scoop install velgrinor` |
| **AUR** | CLI + Desktop | Template | `yay -S velgrinor` / `yay -S velgrinor-launcher-bin` |
| **Flathub** | Desktop | Template | `flatpak install flathub md.thomas.velgrinor.launcher` |

## Automated publishing

Publishing a GitHub release triggers `.github/workflows/store-publish.yml`. Missing repository credentials skip only the corresponding store job; malformed tags, missing artifacts, invalid checksums, and unresolved Git commits fail the job instead of publishing a broken manifest.

The checked-in templates must always carry the same version as `desktop/src-tauri/tauri.conf.json`. Run `node scripts/validate-release.mjs vX.Y.Z` before creating a tag.

## Manual fallback

### Homebrew (Sqrilizz/homebrew-velgrinor)

1. Get SHA256 hashes from the release's `SHA256SUMS.txt`
2. Update `Formula/velgrinor.rb` and `Casks/velgrinor-launcher.rb` with new version and hashes
3. Push to the tap repository

### Winget

1. Fork [microsoft/winget-pkgs](https://github.com/microsoft/winget-pkgs)
2. Copy `winget/` manifest to `manifests/t/Th0rgal/VelGrinorLauncher/<version>/`
3. Update version, URLs, and SHA256 hashes
4. Submit PR

### Scoop

1. Use the `Sqrilizz/scoop-velgrinor` bucket repository
2. Update `velgrinor.json` with new version and hash
3. Push to bucket repository

### AUR

1. Update PKGBUILDs with new version and hashes
2. Use `makepkg --printsrcinfo > .SRCINFO` to regenerate
3. Push to AUR git repositories

### Flathub

1. Fork [flathub/flathub](https://github.com/flathub/flathub) for initial submission
2. Update manifest with new version and hash
3. Submit PR
