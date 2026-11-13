# YouTube Desktop

YouTube as a desktop application, with **real tabs**, a **built-in ad blocker**
and **optional browser extensions**.

Built on [Pake](https://github.com/tw93/Pake) (Tauri 2 + WebView2). Pake turns a
website into a lightweight desktop app, but it gives you a single page. This fork
adds a tab system on top, and narrows the whole shell down to one site: YouTube.

<p align="left">
  <img alt="Platform" src="https://img.shields.io/badge/platform-Windows-0078d4">
  <img alt="License" src="https://img.shields.io/badge/license-GPL--3.0--or--later-blue">
  <img alt="Built with Tauri" src="https://img.shields.io/badge/tauri-2.x-24c8db">
</p>

## Scope: YouTube only

This repository builds **one application**, not a general-purpose site wrapper.
The upstream Pake CLI and multi-app release workflows remain in the tree as
build tooling; the shipped app is locked to YouTube:

- Every tab address goes through an allowlist (`youtube.com`, its subdomains,
  `youtube-nocookie.com`, `youtu.be` and the two Google hosts used by sign-in).
  Anything else falls back to the home page.
- Page navigations and popups outside that allowlist are denied in the webview
  and opened in the system browser instead.
- Saved sessions are revalidated, so a tampered `session.json` cannot turn the
  app into a browser for another site.
- Identity, icon, home page and window title are fixed to YouTube in
  `src-tauri/tauri.conf.json` and `src-tauri/pake.json`.

The allowlist lives in `src-tauri/src/app/youtube.rs` and is mirrored for
page-side links in `src-tauri/src/inject/tab_bridge.js`. Keep both in sync.

## Features

- **Built-in ad blocking** — works on a clean install with no extensions. Ad
  placements are removed from player data, ad requests are answered locally,
  ad slots are hidden, and an ad that slips through is skipped. See
  [Ad blocking](#ad-blocking) and [Updating the filters](#updating-the-filters).
- **Optional extensions** — any unpacked Chrome/Edge extension dropped into
  `extensions/` is loaded per tab, with a settings menu for options pages.
- **Tabs** — open, close, switch and restore. Restored tabs keep their title
  and load only when first opened, so a session of twelve tabs costs one page load.
- **Fluid tab strip** — 200 px is a ceiling, not a size; tabs shrink together
  as the bar fills, down to the favicon alone, then the strip scrolls.
- **Middle click** — on a video link it opens a background tab; on a tab it closes it.
- **Window title follows the active tab**, in the title bar and the taskbar.
- **Fullscreen** — the strip disappears and the video takes the window; `Esc`
  brings it back.
- **Profiles** — `--profile1` … `--profile99`, each with its own cookies, logins
  and session.

## Keyboard shortcuts

| Shortcut                      | Action                |
| ----------------------------- | --------------------- |
| `Ctrl+T`                      | New tab               |
| `Ctrl+W`                      | Close the active tab  |
| `Ctrl+R`                      | Reload the active tab |
| `Ctrl+Tab` / `Ctrl+Shift+Tab` | Next / previous tab   |
| `Ctrl+1` … `Ctrl+8`           | Go to that tab        |
| `Ctrl+9`                      | Go to the last tab    |
| `Ctrl+Shift+E`                | Extensions menu       |
| `Alt+Left` / `Alt+Right`      | Back / forward        |

## Ad blocking

WebView2 does apply uBlock Origin Lite's network rules, but it does **not** apply
its cosmetic filters. Worse, a fresh install has no `extensions/` folder at all,
so a blocker that depends on an extension simply is not there on day one.

`src-tauri/src/inject/adblock.js` is injected into every content webview before
YouTube's own scripts. It works at three levels:

1. **Player data** — `ytInitialPlayerResponse`, `ytInitialData`, `JSON.parse`,
   `Response.json()` and XHR `responseText` are scrubbed of ad keys such as
   `adPlacements`, `playerAds`, `adSlots` and the ad-slot renderers. With no
   placement in the response, no ad break is ever scheduled.
2. **Network** — `fetch`, `XMLHttpRequest.open` and `navigator.sendBeacon` answer
   requests matching ad URL patterns locally, before anything leaves the webview.
3. **Page** — a constructed stylesheet (adopted, so it survives YouTube
   rebuilding its `<head>`) hides feed, search, sidebar and masthead ad slots.
   A `MutationObserver` reapplies it, and a small watchdog clicks the skip button
   or seeks to the end if an ad still plays.

The player's own ad module is deliberately not hidden: if an ad ever plays, its
countdown and skip button must stay usable.

Extensions are a **second, optional layer**. The built-in blocker is on by
default; `YouTube.exe --no-adblock` disables it without touching extensions.

Diagnostics: `window.__ytpakeAdblock` exposes `stats`, the active filter
`version`/`source` and the pure `scrub()` / `isAdRequest()` helpers, so a
DevTools console can confirm what is happening in a tab.

## Updating the filters

The lists are **not** baked into `adblock.js`. They live in
[`filters/youtube.json`](filters/youtube.json), which is read at startup and
injected as `window.__YTPAKE_ADBLOCK_FILTERS__`. When YouTube changes its markup,
replace that JSON and restart — no rebuild, no reinstall.

Lookup order, first valid file wins:

1. `--filters <path>` or `YTPAKE_FILTERS_FILE` (test a candidate).
2. `<app data>/filters/youtube.json` (user update, survives reinstalls).
3. `<exe dir>/filters/youtube.json` (copy shipped by the installer).
4. The catalogue compiled into the binary (always available).

An invalid file is skipped with a line in `%TEMP%\ytpake-trace.log`, and the
blocker continues with the next source. See [filters/README.md](filters/README.md)
for the format and the rules a new filter must obey.

```bash
pnpm check                 # validate filters and extensions before building
pnpm filters:check         # just the filter catalogue
node scripts/check-filters.mjs --file candidate.json
```

The app never fetches filter lists over the network. If you want lists that
refresh themselves, install uBlock Origin Lite (MV3) as an unpacked extension —
it updates its own rulesets on top of the built-in blocker.

## Extensions

Put an **unpacked** extension folder (one containing `manifest.json`) into
`extensions/`. Manifest V3 is recommended; classic uBlock Origin (MV2) can leave
requests hanging in WebView2.

| Build     | Location                                              |
| --------- | ----------------------------------------------------- |
| Portable  | `extensions/` next to `YouTube.exe`                   |
| Installed | `extensions/` inside `%LOCALAPPDATA%\YouTube`         |
| Override  | `--extensions-dir C:\path` or `YTPAKE_EXTENSIONS_DIR` |

The folder is copied into the installer by `tauri.conf.json`
(`bundle.resources`), so add the extension **before** building. Every profile
shares the same folder. Nothing is hard-coded: any folder with a manifest is
loaded, and any one with an options page joins the puzzle-piece menu
(`Ctrl+Shift+E`). The app patches each options page to be web-accessible from
YouTube so it can be embedded in the tab strip.

```bash
pnpm extensions:check      # names, versions, MV level, manifest problems
YouTube.exe --no-extensions
```

Extension content scripts are registered per webview by WebView2, one renderer
at a time. Tabs are therefore created a few seconds apart and each first loads a
cheap 404 page on the same origin before its real address.
[docs/architecture.md](docs/architecture.md) has the measurements.

Ad blocking does not depend on this mechanism. If WebView2 refuses an extension
folder, the app drops extensions for that session and the built-in blocker keeps
working; the startup trace records what happened.

## Requirements

- Windows 10 or 11 with the WebView2 runtime (present by default on current
  Windows; otherwise the
  [Evergreen Bootstrapper](https://developer.microsoft.com/microsoft-edge/webview2/)).
- [Node.js](https://nodejs.org) 20.9 or newer and [pnpm](https://pnpm.io).
- [Rust](https://rustup.rs) with a toolchain for your target. On Windows both
  `stable-x86_64-pc-windows-msvc` and `stable-x86_64-pc-windows-gnu` work.

## Build

```bash
pnpm install
pnpm check
pnpm app:build
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

| Path                                                               | What it is      |
| ------------------------------------------------------------------ | --------------- |
| `src-tauri/target/release/YouTube.exe`                             | The application |
| `src-tauri/target/release/bundle/nsis/YouTube_1.0.0_x64-setup.exe` | Installer       |

### Portable copy

Put the executable in a folder with an empty `portable` file beside it and the
profile is written next to the executable instead of `%APPDATA%`:

```
YouTube/
├─ YouTube.exe
├─ portable            (empty file — switches portable mode on)
├─ filters/youtube.json
├─ extensions/         (optional)
└─ profile1/webview/   (created on first run)
```

Without the marker the app stores its profile under
`%LOCALAPPDATA%\com.pake.youtube.tabs`.

## Command-line flags

| Flag                                           | Effect                                            |
| ---------------------------------------------- | ------------------------------------------------- |
| `--profileN`                                   | Use profile 1–99 (separate cookies, tabs, window) |
| `--no-extensions`                              | Start without any unpacked extension              |
| `--extensions-dir <path>`                      | Load extensions from another folder               |
| `--no-adblock`                                 | Disable only the built-in blocker                 |
| `--filters <path>`                             | Use a specific filter catalogue for this run      |
| `YTPAKE_FILTERS_FILE`, `YTPAKE_EXTENSIONS_DIR` | Environment equivalents                           |

## Localisation

The interface is Brazilian Portuguese. The strings live in `ui/tabs.html` and
`src-tauri/src/inject/tab_bridge.js`, so a translation means editing those two
files. Documentation and code comments are in English.

## Documentation

- [docs/user-guide.md](docs/user-guide.md) — the readme that ships with the app.
- [docs/architecture.md](docs/architecture.md) — the tab system, extension
  warm-up and WebView2 behaviour.
- [filters/README.md](filters/README.md) — updating the filter catalogue.
- [extensions/README.md](extensions/README.md) — installing extensions.

## Credits

A fork of [Pake](https://github.com/tw93/Pake) by Tw93, which does the hard part:
a small, fast Tauri shell around a webview. The tab system, the YouTube-only
policy, the built-in blocker, profile handling and extension plumbing are the
changes on top.

Loaded at runtime, not part of this repository:
[uBlock Origin Lite](https://github.com/uBlockOrigin/uBOL-home) and
[Enhancer for YouTube](https://github.com/maximelebreton/Enhancer-for-YouTube).

## License

GPL-3.0-or-later, as Pake. See [LICENSE](LICENSE).
