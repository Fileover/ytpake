# YouTube Desktop

YouTube as a desktop application, with **real tabs**.

Built on [Pake](https://github.com/tw93/Pake) (Tauri 2 + WebView2). Pake turns a
website into a lightweight desktop app, but it gives you a single page. This fork
adds a tab system on top of it: a tab strip with one webview per tab, session
restore, and the extensions wired so they apply to every tab.

<p align="left">
  <img alt="Platform" src="https://img.shields.io/badge/platform-Windows-0078d4">
  <img alt="License" src="https://img.shields.io/badge/license-GPL--3.0--or--later-blue">
  <img alt="Built with Tauri" src="https://img.shields.io/badge/tauri-2.x-24c8db">
</p>

## Why this exists

A browser is a tabbed interface; a wrapped site is not. Opening a video from the
feed and losing the feed is the whole problem, and a single webview makes that
worse than the browser. So this fork puts a tab strip above the page and one
webview per tab — each with its own history, its own scroll position and its own
media playback.

## Features

- **Tabs** — open, close, switch and restore. Restored tabs keep their title and
  load only when first opened, so a session of twelve tabs costs one page load.
- **Fluid tab strip** — 200 px is a ceiling, not a size. Tabs shrink together as
  the bar fills, down to the favicon alone, then the strip scrolls horizontally
  (the mouse wheel scrolls it too).
- **Middle click** — on a video link it opens the video in a background tab and
  leaves you where you were; on a tab it closes that tab.
- **Window title follows the active tab**, in the title bar and the taskbar.
- **Fullscreen** — the strip disappears and the video takes the window; `Esc`
  brings it back with the tab where you left it.
- **Bundled extensions** — uBlock Origin Lite and Enhancer for YouTube, loaded
  per tab (see [Extensions](#extensions)).
- **Profiles** — `--profile1` … `--profile99`, each with its own cookies, logins
  and session, so two accounts can run side by side.

## Keyboard shortcuts

| Shortcut | Action |
| --- | --- |
| `Ctrl+T` | New tab |
| `Ctrl+W` | Close the active tab |
| `Ctrl+R` | Reload the active tab |
| `Ctrl+Tab` / `Ctrl+Shift+Tab` | Next / previous tab |
| `Ctrl+1` … `Ctrl+8` | Go to that tab |
| `Ctrl+9` | Go to the last tab |
| `Ctrl+Shift+E` | Extensions menu |
| `Alt+Left` / `Alt+Right` | Back / forward |

## Requirements

- Windows 10 or 11 with the WebView2 runtime (present by default on current
  Windows; otherwise the
  [Evergreen Bootstrapper](https://developer.microsoft.com/microsoft-edge/webview2/)).
- [Node.js](https://nodejs.org) 20.9 or newer and [pnpm](https://pnpm.io).
- [Rust](https://rustup.rs) with a toolchain for your target. On Windows both
  `stable-x86_64-pc-windows-msvc` and `stable-x86_64-pc-windows-gnu` work; the
  build below was made with the GNU one.

## Build

```bash
pnpm install
npx @tauri-apps/cli@2 build --bundles nsis
```

With the MinGW toolchain, tell Cargo which one to use:

```bash
RUSTUP_TOOLCHAIN=stable-x86_64-pc-windows-gnu \
CARGO_BUILD_JOBS=2 \
CARGO_PROFILE_RELEASE_LTO=false \
CARGO_PROFILE_RELEASE_OPT_LEVEL=2 \
CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 \
npx @tauri-apps/cli@2 build --bundles nsis
```

`LTO=false` and `OPT_LEVEL=2` trade a little speed for a build that finishes in
minutes instead of tens of minutes. Drop them for a release build.

Output:

| Path | What it is |
| --- | --- |
| `src-tauri/target/release/YouTube.exe` | The application |
| `src-tauri/target/release/bundle/nsis/YouTube_1.0.0_x64-setup.exe` | Installer |

### Portable copy

Put the executable in a folder with an empty `portable` file beside it and the
profile is written next to the executable instead of `%APPDATA%`, which makes the
folder self-contained:

```
YouTube/
├─ YouTube.exe
├─ portable            (empty file — switches portable mode on)
├─ extensions/         (uBlock Origin Lite, Enhancer for YouTube)
└─ profile1/webview/   (created on first run)
```

Without the marker the app stores its profile under
`%LOCALAPPDATA%\com.pake.youtube.tabs`.

### Extensions

The application expects two extension folders next to the executable and runs
without them if they are missing:

```
extensions/
├─ ublock-origin-lite/     https://github.com/uBlockOrigin/uBOL-home        (GPL-3.0)
└─ enhancer-for-youtube/   https://github.com/maximelebreton/Enhancer-for-YouTube
```

Download the unpacked release of each and keep the folder names. Pass
`--no-extensions` to start without loading any of them, which is the quickest way
to tell an extension problem from an application one.

They are not vendored here: they are separate GPL-3.0 projects, and shipping them
belongs in a release archive rather than in the source tree.

### Profiles

```bash
YouTube.exe --profile2                     # a second account, separate cookies and session
YouTube.exe --profile2 --no-extensions
```

## Extensions

Two things had to be solved to make extensions usable here, and both are WebView2
behaviour rather than Pake's:

**Content scripts are registered per webview, one at a time.** A renderer created
before the extension host is ready never receives them, and four tabs created at
once all end up without the scripts. Tabs are therefore created a few seconds
apart, and each one first loads a throwaway page on the site — an HTML page, not
`robots.txt`, which arrives as `text/plain` and never runs content scripts at
all. [docs/architecture.md](docs/architecture.md) has the measurements.

**uBlock's cosmetic filtering does nothing in WebView2.** Network rules work — an
ad script really is cancelled — but an element EasyList is supposed to hide keeps
its box, before and after a reload. The ads that survive are the ones YouTube
renders into the page itself, so the application hides those slots with its own
stylesheet (`src-tauri/src/inject/hide_ads.js`). The player's own ad UI is left
alone on purpose: blocking covers it, and hiding it when an ad does slip through
would leave a black player with no countdown and no skip button.

## Localisation

The interface is Brazilian Portuguese. The strings live in `ui/tabs.html` and
`src-tauri/src/inject/tab_bridge.js`, so a translation means editing those two
files.

## Documentation

- [docs/user-guide.md](docs/user-guide.md) — what the app does, in the words of
  the readme that ships with it.
- [docs/architecture.md](docs/architecture.md) — the tab system, the extension
  warm-up, and the WebView2 behaviour behind both.

## Credits

A fork of [Pake](https://github.com/tw93/Pake) by Tw93, which does the hard part:
a small, fast Tauri shell around a webview. The tab system, the profile handling
and the extension plumbing are the changes on top. Pake's own documentation is
kept in [README_PAKE.md](README_PAKE.md) — the `pake` CLI still works as
documented there.

Loaded at runtime, not part of this repository:
[uBlock Origin Lite](https://github.com/uBlockOrigin/uBOL-home) and
[Enhancer for YouTube](https://github.com/maximelebreton/Enhancer-for-YouTube).

## License

GPL-3.0-or-later, as Pake. See [LICENSE](LICENSE).
