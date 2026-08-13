<p align="center">
  <img src="logo.png" alt="VelGrinor" width="180" height="180">
</p>

<h1 align="center">VelGrinor</h1>

<p align="center">
  <strong>A fast, transparent Minecraft launcher for modded and vanilla profiles.</strong><br>
  Microsoft and offline accounts, modpacks, a shared content library, and clear launch progress.
</p>

<p align="center">
  <a href="README.md">English</a> · <a href="README_RU.md">Русский</a>
</p>

<p align="center">
  <a href="https://github.com/Sqrilizz/VelGrinor/actions/workflows/ci.yml"><img src="https://img.shields.io/github/actions/workflow/status/Sqrilizz/VelGrinor/ci.yml?branch=main&style=flat-square&label=build&color=00a331" alt="Build status"></a>
  <a href="https://github.com/Sqrilizz/VelGrinor/releases"><img src="https://img.shields.io/github/v/release/Sqrilizz/VelGrinor?include_prereleases&style=flat-square&color=00e07c" alt="Latest release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-007103?style=flat-square" alt="MIT License"></a>
  <img src="https://img.shields.io/badge/platforms-Windows%20%7C%20Linux%20%7C%20macOS-004d00?style=flat-square" alt="Windows, Linux and macOS">
</p>

<p align="center">
  <img src="screenshot.webp" alt="VelGrinor library" width="900">
</p>

## Built for actual play

VelGrinor combines a polished Tauri desktop app with a complete Rust CLI. Profiles are stored as readable manifests, while identical mods, resource packs, and shaders are deduplicated by SHA-256 instead of being copied for every instance.

| Play | Build | Recover |
|---|---|---|
| Microsoft and offline accounts | Modrinth and CurseForge catalog | Profile snapshots and rollback |
| Fabric, Forge, NeoForge, and Quilt | `.mrpack` and CurseForge ZIP import | Crash diagnostics and repair actions |
| English and Russian interface | Mods, shaders, packs, and modpacks | Logs and preparation diagnostics |
| Discord Rich Presence | Real project icons and source links | Exportable, reproducible profiles |

### Progress you can trust

Downloads and game preparation report the current stage, percentage, speed, and estimated time where the source provides enough information. Installing a modpack no longer looks frozen while large files are being processed.

### One shared library

Content is addressed by its SHA-256 hash. If ten profiles use the same file, VelGrinor stores it once and materializes clean game instances from the profile definitions.

### Native desktop experience

The launcher supports Windows, Linux, and macOS, includes automatic update checks, platform-aware Java discovery, skin previews, bilingual UI, and Discord RPC. Linux launch handling preserves the graphics environment so Java can use the selected GPU and driver normally.

## Screenshots

| Library | Store |
|---|---|
| ![Library](web/public/screenshots/library.webp) | ![Store](web/public/screenshots/store.webp) |

| Profile content | Settings |
|---|---|
| ![Profile content](web/public/screenshots/overview.webp) | ![Settings](web/public/screenshots/settings.webp) |

## Installation

Ready-to-use packages are published on the [Releases page](https://github.com/Sqrilizz/VelGrinor/releases):

- Windows: `.msi` or `.exe`
- Linux: `.AppImage` or `.deb`
- macOS: `.dmg`

### Build from source

Requirements: current stable Rust, Node.js 22, and the native Tauri dependencies for your platform.

```bash
git clone https://github.com/Sqrilizz/VelGrinor.git
cd VelGrinor

# CLI
cargo build --release -p velgrinor

# Desktop application
cd desktop
npm ci
npm run tauri:build
```

The desktop bundles are written to `target/release/bundle/`.

## Quick start

```bash
# Sign in through Microsoft, or create an offline account
velgrinor account add
velgrinor account offline PlayerName

# Create and launch a profile
velgrinor profile create performance --mc 1.21.4 --loader fabric
velgrinor mod add performance sodium
velgrinor launch performance
```

Useful entry points:

```bash
velgrinor library
velgrinor store search sodium
velgrinor modpack import ./pack.mrpack
velgrinor logs
velgrinor account list
velgrinor --help
```

## External services

Microsoft sign-in needs a sign in by mail and password (like other launchers). Offline accounts work without Microsoft authentication.

CurseForge browsing needs an API key, which can be entered in Settings or supplied as `VELGRINOR_CURSEFORGE_API_KEY`. Modrinth integration does not require a key.

Discord Rich Presence uses application ID `1521208567036645426` and can be disabled in Settings.

## Development

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

cd desktop
npm ci
npm run check
npm run build
```

The release workflow builds platform packages through GitHub Actions. Contributions and reproducible bug reports are welcome.

## Links

- [Releases](https://github.com/Sqrilizz/VelGrinor/releases)
- [Issue tracker](https://github.com/Sqrilizz/VelGrinor/issues)
- [Discord](https://discord.gg/2ng6q3JNQ7)
- [Creator — Sqrilizz](https://sqrilizz.tech)

## License

[MIT](LICENSE) © Sqrilizz
