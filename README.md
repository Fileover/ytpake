# ytpake

[![Build](https://github.com/Fileover/ytpake/actions/workflows/build.yml/badge.svg)](https://github.com/Fileover/ytpake/actions/workflows/build.yml)

**ytpake** is a lightweight, YouTube-only desktop application for Windows built on **Rust**, **Tauri 2**, and **Microsoft Edge WebView2**. It provides a multi-tab interface with isolated profiles and native unpacked browser extension support loaded directly from `.\extensions`.

---

## Key Features

- **Multi-Tab Interface (`ui/tabs.html`)**
  - Dedicated WebView2 instance per tab with independent history, scroll position, and media playback.
  - Automatic session persistence (`tabs.json`) and window geometry restoration (`window-state.json`).
  - Full-screen mode and middle-click / `Ctrl+Click` link routing into background tabs.
- **Strict YouTube-Only Navigation Policy**
  - Restricts top-level tab navigation to YouTube domains (`youtube.com`, `youtube-nocookie.com`, `youtu.be`) and Google sign-in endpoints (`accounts.google.com`, `consent.google.com`).
  - External links are automatically blocked inside the app and opened in the system default browser.
- **Fast External Extensions (`.\extensions`)**
  - No built-in extensions or bundled filter lists; loads unpacked Chromium extensions exclusively from the `.\extensions` folder next to `YouTube.exe` (or via `--extensions-dir <path>`).
  - Synchronizes extensions with the WebView2 profile **once per session** via `ICoreWebView2Profile7` (`GetBrowserExtensions` / `AddBrowserExtension`), reusing WebView2's persistent extension registry and pre-indexed `declarative_net_request` FlatBuffers instead of reinstalling on every tab creation.
  - Automatically strips `update_url` and grants YouTube origin access to Manifest V3 extension options pages so extension settings open cleanly inside tabs via the top-right gear menu.
- **Isolated Profiles & Portable Mode**
  - Pass `--profile1` through `--profile99` to run separate accounts, cookies, extensions, and tab sessions.
  - Create an empty `portable` or `portable.txt` file next to `YouTube.exe` (or set `PAKE_PORTABLE=1`) to store all profile data in `.\profileN` next to the executable.

---

## Keyboard Shortcuts

| Shortcut | Action |
| :--- | :--- |
| `Ctrl+T` | Open a new YouTube tab |
| `Ctrl+W` / `Ctrl+F4` | Close the active tab |
| `Ctrl+Tab` / `Ctrl+PageDown` | Switch to the next tab |
| `Ctrl+Shift+Tab` / `Ctrl+PageUp` | Switch to the previous tab |
| `Ctrl+1` .. `Ctrl+8` | Jump to tab 1–8 |
| `Ctrl+9` | Jump to the last tab |
| `Alt+Left` / `Alt+Right` | Navigate back / forward in the active tab |
| `Ctrl+R` / `F5` | Reload the active tab |
| `F11` | Toggle full-screen mode |
| `Middle-Click` / `Ctrl+Click` | Open a YouTube link in a new background tab |

---

## Directory Layout

### Runtime Layout (Portable / Installed)

```text
YouTube/
├── YouTube.exe
├── portable                  # Optional marker file enabling portable mode
├── extensions/               # Unpacked Chromium extensions (each with a manifest.json)
│   ├── adguard/
│   └── enhancer-for-youtube/
└── profile1/                 # Created automatically in portable mode
    ├── EBWebView/            # WebView2 user data & persisted extension state
    ├── tabs.json             # Saved tab session
    └── window-state.json     # Saved window size and position
```

### Repository Layout

```text
ytpake/
├── .github/workflows/
│   └── build.yml             # GitHub Actions CI workflow (tests + release build)
├── src-tauri/
│   ├── capabilities/
│   │   └── default.json      # Tauri IPC capability permissions
│   ├── png/                  # Application icons
│   ├── src/
│   │   ├── app/
│   │   │   ├── extensions.rs # Unpacked extension discovery, manifest prep & WebView2 profile sync
│   │   │   ├── mod.rs
│   │   │   ├── profile.rs    # Profile selection (--profile1..99), portable mode & path resolution
│   │   │   ├── tabs.rs       # Multi-webview tab manager, IPC commands & session persistence
│   │   │   └── youtube.rs    # YouTube-only URL allowlist and navigation policy
│   │   ├── inject/
│   │   │   └── tab_bridge.js # Keyboard shortcut & link-click bridge injected into tabs
│   │   ├── lib.rs
│   │   └── main.rs
│   ├── build.rs
│   ├── Cargo.lock
│   ├── Cargo.toml
│   └── tauri.conf.json
├── ui/
│   └── tabs.html             # Top tab strip UI and extension settings menu
└── rust-toolchain.toml
```

---

## Command-Line Arguments

| Argument | Description |
| :--- | :--- |
| `--profile<1..99>` | Select an isolated profile slot (defaults to `profile1`). |
| `--extensions-dir <path>` | Load unpacked extensions from a custom directory instead of `.\extensions`. |

---

## Building from Source

### Prerequisites

- **OS:** Windows 10 / 11 (x64) with Microsoft Edge WebView2 Runtime installed
- **Rust:** `stable-x86_64-pc-windows-msvc` toolchain
- **Visual Studio C++ Build Tools** (Desktop development with C++)

### Local Build

```powershell
# Run unit tests
cargo test --manifest-path src-tauri/Cargo.toml --lib

# Build optimized release binary (src-tauri/target/release/YouTube.exe)
cargo build --manifest-path src-tauri/Cargo.toml --release --bin YouTube
```

### GitHub Actions CI

The repository includes a GitHub Actions workflow at [`.github/workflows/build.yml`](.github/workflows/build.yml) that automatically:

1. Runs the unit test suite (`cargo test --lib`) on `windows-latest`.
2. Compiles the stripped, LTO-optimized `YouTube.exe` release binary.
3. Uploads `YouTube.exe` as the `YouTube-windows-x64` workflow artifact on every push, pull request, or manual dispatch.
4. Publishes `YouTube.exe` to GitHub Releases whenever a `v*` / `V*` tag is pushed.
