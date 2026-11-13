# User guide

This is the readme that ships with the application, in English.

## Files

| File                          | What it is                              |
| ----------------------------- | --------------------------------------- |
| `YouTube_1.0.0_x64-setup.exe` | Installer (Windows x64, NSIS, per-user) |
| `YouTube-portable\`           | Portable build — installs nothing       |

## Portable build

The `YouTube-portable` folder already contains the `portable` marker. With it,
everything is written next to the executable instead of `%APPDATA%`:

```
YouTube-portable\
  YouTube.exe
  WebView2Loader.dll
  portable                 portable mode marker
  filters\youtube.json     ad filter catalogue (editable, no rebuild needed)
  extensions\              optional extensions (shared by every profile)
  profile1\                created on first use
  profile2\ ...
```

You can copy the folder to a USB stick and run it on another machine. The WebView2
runtime has to be installed there (Windows 11 ships with it).

## Profiles

```bash
YouTube.exe                # profile 1 (default)
YouTube.exe --profile2     # profile 2
YouTube.exe --profile99    # up to 99
```

Each profile has its own cookies, logins, tabs, history and window geometry.
Different profiles run at the same time, each with its own taskbar button.

In the installed version profiles live in `%APPDATA%\com.pake.youtube.tabs\`.

## Extensions

`extensions\` takes unpacked extensions — a folder with a `manifest.json` inside.
It is a single folder shared by every profile.

| Build     | Location                             |
| --------- | ------------------------------------ |
| Portable  | `extensions\` next to `YouTube.exe`  |
| Installed | `%LOCALAPPDATA%\YouTube\extensions\` |

The puzzle-piece button in the tab strip opens the settings of any extension that
declares an options page. With one extension it opens directly; with two or more
it opens a menu. Shortcut: `Ctrl+Shift+E`.

The options page appears as a panel over the current tab. Close it with the X in
the corner of the panel or by clicking outside it (`Esc` does not close it: focus
stays inside the extension's page).

The button only appears when at least one extension with an options page exists.
Nothing is hard-coded: any folder in `extensions\` with a `manifest.json` and an
options page joins the menu, under the name the extension itself declares. The
button stays pinned to the right edge of the tab strip.

To disable every extension without deleting anything: `YouTube.exe --no-extensions`.
To load extensions from another folder, use `YouTube.exe --extensions-dir C:\ext`
or set `YTPAKE_EXTENSIONS_DIR`. Before building, run `pnpm extensions:check` to
validate the manifests; at runtime the same inventory is written to
`%TEMP%\ytpake-trace.log` with the prefix `extensions:`.

Extensions are optional. The application's own ad blocker works with an empty
`extensions\` folder; see below.

## Tabs

A tab is at most 200 px wide. With few tabs open each one sits at 200 px; as the
bar fills they shrink together and stop at the icon, without the name. At that
size the close X takes the icon's place on hover. When they do not fit even as
icons, the strip scrolls horizontally — the mouse wheel over the tabs scrolls it
too.

Saved tabs come back with their title but only load when they are first opened.
Besides making startup a single page load, this is what gets the extensions into
every tab: WebView2 registers an extension's content scripts with one tab at a
time, so opening several at once left most of them without the scripts.

Middle click (scroll wheel) on a video opens one tab in the background: the page
you are on stays in front. On a tab, the same click closes it.

The window title follows the active tab, in the title bar and the taskbar.

## About the ad blocker

Ad blocking is built in. It works on a fresh install with an empty `extensions\`
folder — no download, no setup, no extension required.

The app removes ad placements from YouTube's player data before the page sees it,
answers known ad requests locally, hides the ad slots YouTube draws into the feed,
search results and watch sidebar, and skips an ad in the player if one still
plays. The player's own ad UI is left visible on purpose so a countdown and a
skip button remain usable.

To turn the built-in blocker off: `YouTube.exe --no-adblock`. This does not
affect any extension.

### Updating the filters

The filter lists live in `filters\youtube.json`, not in the executable's code.
When YouTube changes its markup, replace that file and restart the application —
there is no need to rebuild or reinstall it.

| Build        | Where to put an updated file                           |
| ------------ | ------------------------------------------------------ |
| Portable     | `YouTube-portable\filters\youtube.json`                |
| Installed    | `%APPDATA%\com.pake.youtube.tabs\filters\youtube.json` |
| One-off test | `YouTube.exe --filters C:\path\youtube.json`           |

An invalid file is ignored and the app falls back to the copy shipped with the
installer, then to the version compiled into the binary. The active source and
version are written to `%TEMP%\ytpake-trace.log` on every launch. Before
deploying a new file, run `pnpm filters:check` (or
`node scripts/check-filters.mjs --file new.json`).

The app does not download filter lists by itself. For lists that refresh
automatically, add uBlock Origin Lite (Manifest V3) as an unpacked extension —
it updates its own rulesets on top of the built-in blocker. Do not use classic
uBlock Origin (Manifest V2): in WebView2 it leaves requests hanging, so
thumbnails never finish loading and new tabs sometimes come up blank.

## Fullscreen

With the video fullscreen (`F` in the player) the tab strip disappears and the
video takes the whole window. On the way out (`Esc`) the strip comes back and the
tab is where it was.

## Troubleshooting

- Every tab takes half a second longer to show its page: before going to the real
  address it opens a 404 page on the same site, which is what makes WebView2
  register the extension for that document. Without that step the tab came up
  without Enhancer and without the blocker. With extensions disabled the step does
  not exist.
- Blank page in a new tab: the tab reloads itself once after 8 s. If it persists,
  run with `--no-extensions` to confirm it is an extension.
- On first use WebView2 has to load the extension, so the page takes a few seconds
  and the extensions button only appears once it opens. From the second time on it
  is fast.
- Ads still showing: the blocker is on by default, so check
  `%TEMP%\ytpake-trace.log` for the `adblock filters:` line and confirm which
  filter version is active. With remote debugging enabled on port 9222,
  `python tools/adblock_probe.py` reports the blocker's live counters and any
  visible ad slot. Replacing `filters\youtube.json` with a newer catalogue is
  the usual fix when YouTube changes its markup.
- To see what the application is doing: `%TEMP%\ytpake-trace.log`, one line per
  startup step, with the time in milliseconds.
