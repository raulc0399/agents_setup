# Prism desktop app: build and install

The app is built on the machine it runs on: one command on the Mac, one on the Windows PC.
The builds are unsigned and there is no CI. The frontend is plain HTML, CSS and JavaScript,
so no Node is needed.

## Prerequisites

| | macOS | Windows |
| --- | --- | --- |
| Compiler | Xcode Command Line Tools: `xcode-select --install` | Visual Studio 2022 Build Tools, workload "Desktop development with C++" |
| Rust | [rustup](https://rustup.rs), Rust 1.99 or newer | rustup with the default MSVC toolchain, Rust 1.99 or newer |
| Tauri CLI | `cargo install tauri-cli --version "^2" --locked` | the same |
| Web view | built in (WKWebView) | WebView2, part of Windows 10 and 11; the installer fetches it if missing |

The first Windows build downloads NSIS into the Tauri cache, so it needs internet access.

## Icons (once, on the Mac)

`src-tauri/icons/icon.png` (1024 x 1024) is the source. From `features/prism/app/src-tauri`:

```sh
cargo tauri icon icons/icon.png
```

This writes the icon set into `src-tauri/icons/`. Commit only the files that `bundle.icon` in
`tauri.conf.json` lists (`32x32.png`, `128x128.png`, `128x128@2x.png`, `icon.icns`,
`icon.ico`), so the Windows build uses the same set; leave the other generated files out.
Until then the bundles carry the older placeholder set.

## Build

From `features/prism/app/src-tauri`:

```sh
cargo tauri build
```

The first `.dmg` build asks to let the terminal control Finder (it lays out the disk image
window); allow it, since denying it fails the build.

| Platform | Artifacts in `features/prism/app/target/release/bundle/` |
| --- | --- |
| macOS | `macos/Prism.app`, `dmg/Prism_<version>_<arch>.dmg` |
| Windows | `nsis/Prism_<version>_x64-setup.exe` |

Bundle settings: shared ones in `src-tauri/tauri.conf.json`, per platform in
`tauri.macos.conf.json` (targets `app` and `dmg`, macOS 11 or newer) and
`tauri.windows.conf.json` (target `nsis`, per-user install, WebView2 download
bootstrapper). Nothing is signed or notarized.

The version is `version` in `Cargo.toml` (`[workspace.package]`) and in
`src-tauri/tauri.conf.json`; change both. The host test
`the_tauri_config_carries_the_workspace_version` fails while they differ.

## Install on a clean user account

**macOS**: open the `.dmg` and drag Prism to Applications. A copy built on this Mac opens
directly. A copy that came through a browser, AirDrop or mail is quarantined: on the first
open macOS says it cannot verify the app; open System Settings → Privacy & Security and click
"Open Anyway" (macOS 14 and older: Control-click the app, Open). If macOS calls the app
damaged, clear the quarantine flag: `xattr -dr com.apple.quarantine /Applications/Prism.app`.

**Windows**: run `Prism_<version>_x64-setup.exe`; it installs for the current user without
administrator rights into `%LOCALAPPDATA%\Prism`. A copy built on this PC runs directly; one
that came through a browser or from another machine makes SmartScreen warn about an unknown
publisher: click "More info", then "Run anyway".

The account needs `~/.claude` or `~/.codex` (Windows: `%USERPROFILE%\.claude`,
`%USERPROFILE%\.codex`), mounted into the containers; the app writes its pairing file only
where the directory exists. The empty window lists the listening address, the pairing
files, any problem with a bridge and, at the bottom, the app version and protocol number.

## Files

| What | macOS | Windows |
| --- | --- | --- |
| Settings, window arrangement, window positions | `~/Library/Application Support/local.prism.desktop/` | `%APPDATA%\local.prism.desktop\` |
| Web view data | `~/Library/WebKit/local.prism.desktop/`, `~/Library/Caches/local.prism.desktop/` | `%LOCALAPPDATA%\local.prism.desktop\` |
| Pairing files | `~/.claude/prism/app.json`, `~/.codex/prism/app.json` | `%USERPROFILE%\.claude\prism\app.json`, `%USERPROFILE%\.codex\prism\app.json` |

The data directory holds `settings.json` (optional, written only by you), `arrangement.json`
and `.window-state.json`. Settings keys, both optional:

```json
{ "bind_address": "127.0.0.1", "port": 7717 }
```

`bind_address` `0.0.0.0` is for plain Linux Docker, where containers do not reach the host's
loopback; the firewall may then ask to allow incoming connections. The app writes the
pairing files (`{port, token}`, mode 0600) at every start and removes them when it quits.

## Versions

The app and the bridge (`prism` in the container) speak a numbered protocol
(`APP_PROTOCOL` in `prism-core/src/protocol.rs`). Different numbers refuse each other, and
both sides name both numbers and the older side: the app in its empty window and Sessions
panel until a bridge connects, the bridge in `/prism`. Update the older side: a newer
`prism` binary in the container (then restart the conversation), or a newer app build.

## Uninstall

**macOS**: quit Prism, delete `/Applications/Prism.app` and the data and web view
directories above. If present (check on the Mac), also delete
`~/Library/HTTPStorages/local.prism.desktop` and
`~/Library/Saved Application State/local.prism.desktop.savedState`.

**Windows**: Settings → Apps → Installed apps → Prism → Uninstall; tick "Delete the
application data" to remove `%APPDATA%\local.prism.desktop` and
`%LOCALAPPDATA%\local.prism.desktop`.

Pairing files left behind (only after a crash) can be deleted; bridges then wait quietly.
