# Architecture

How the tab system is put together, and the WebView2 behaviour that shaped it.
The numbers below are measurements from a Windows 11 machine with WebView2 156,
not estimates.

## The shape of the window

Two webviews and no native chrome:

```
┌──────────────────────────────────────────────────┐
│ strip webview (ui/tabs.html)                     │  ← back, forward, reload,
│  ┌────┐ ┌────┐ ┌────┐              [+][gear]     │    new tab, extensions
├──────────────────────────────────────────────────┤
│ content webview for tab 0                        │
│                                                  │
│ content webview for tab 1   (hidden)             │
│                                                  │
│ content webview for tab 2   (hidden)             │
└──────────────────────────────────────────────────┘
```

Every tab is a real `Webview` created inside the same window and laid out to the
content rectangle; only the active one is shown. Hiding the others is what keeps
a background tab from painting over the page you are reading, and it is also the
source of the first trap below.

The strip is a plain HTML page — no framework. It talks to Rust through Tauri's
`invoke` and renders whatever `tabs_state` returns.

Data directories are separate on purpose, so the strip's cache never competes
with the page's:

| Webview | Profile location                                 |
| ------- | ------------------------------------------------ |
| strip   | `%LOCALAPPDATA%\com.pake.youtube.tabs\EBWebView` |
| content | `<base>\profile1\webview\EBWebView`              |

`<base>` is the executable's folder when a `portable` marker file sits next to it,
and `%APPDATA%\com.pake.youtube.tabs` otherwise.

## Tab state

Rust owns the list; the strip is a view of it.

```rust
struct TabEntry {
    id: u32,
    title: String,
    url: String,
    loading: bool,
    started: bool,   // the real page began loading (not the warm-up document)
    retried: bool,   // the stall watchdog already fired once
}
```

`session.json`, next to the profile, holds `{ tabs, titles, active }`. Titles are
saved so a restored window shows real names immediately; the pages themselves
load when a tab is first opened. That is what makes restoring twelve tabs cheap,
and it is also what keeps the extension registration from being a stampede —
twelve webviews created at once would all come up without content scripts.

## Trap 1 — content scripts are registered per webview

WebView2 hands an extension's content scripts to a webview **once**, when that
webview's renderer is created, and it cannot do it for several webviews at the
same time. Four tabs created together all end up without the scripts; the same
tabs created a few seconds apart all keep them.

Two consequences, both in `tabs.rs`:

- `TAB_SPACING` (4 s) serialises webview creation. A tab asked for too soon waits
  its turn — the trace logs `slot: waiting <n>ms`.
- Each tab loads a throwaway document first, then its real address. The hop
  exists because the renderer is created for the throwaway document, and by the
  time the real navigation starts the host has had its chance to register.

The throwaway document has three requirements, and the third one cost the most
to find:

1. **Same origin** as the destination, so the extensions treat it as the site.
2. **HTML.** `robots.txt` is the cheapest thing a site serves and it does bring
   the renderer up, but it arrives as `text/plain` and content scripts do not run
   there at all — it warmed the renderer and never the extensions.
3. **A different address from the destination.** A warm-up document at the same
   address is not a second load: the page keeps whatever the extension worker
   handed out while it was still starting. The home page kept its ads until a
   reload for exactly this reason.

A page that does not exist satisfies all three for the price of a 404. On YouTube
that is 758 bytes of `text/html` answered in about 190 ms, against a couple of
seconds for the home page it replaced. `WARMUP_PATH` in `tabs.rs` is that path,
shared by the navigator and by the title guard that keeps a restored tab from
being renamed "404 Not Found".

The wait after the document has loaded is 500 ms. Measured: 400 ms held over two
runs of three tabs, zero is where tabs start losing the extension, and the value
shipped is the one with margin. `PAKE_WARMUP_DELAY_MS` overrides it, which is how
that threshold was found without rebuilding.

Startup on this machine, before and after the 404 warm-up page:

| Milestone    | Home page warm-up | 404 warm-up |
| ------------ | ----------------- | ----------- |
| Warm-up over | 7424 ms           | **5949 ms** |
| First title  | 8021 ms           | **6602 ms** |

## Trap 2 — a navigation to a hidden webview is dropped

Not queued: dropped. A background tab (middle click, or a new tab the strip
created without activating) could issue its navigation while hidden and then sit
on the throwaway document for good, because the tab is hidden _because_ it is
still warming, and it stops warming only when the real page starts loading.

`apply_active` therefore calls `resume_target()`: when a tab is shown without
ever having reached its destination, the navigation is issued again. Activating a
stalled tab recovers it.

## Trap 3 — uBlock's cosmetic filtering does nothing here

uBlock Origin Lite loads, its service worker runs, seven rulesets are enabled and
`<all_urls>` is granted. Network rules work: importing an ad script as a module
rejects, and the same URL fetched with `fetch()` returns 200 — a rule written for
`script` does not match a request of type `xmlhttprequest`, which is a trap of its
own when testing.

Cosmetic filtering does not work. Elements EasyList hides (`.adsbygoogle`,
`.ad-banner`, `#google_ads_frame1`, `.textads`) keep their box, before **and**
after a reload. `testMatchOutcome` matches rule 5229 of `ublock-filters` for the
ad script, so the rules are loaded — they simply do not reach the page.

And an unpacked extension is optional. A clean install has no `extensions/`
folder at all, so the very first run used to have no blocker whatsoever. Hiding a
few slots with CSS could not fix either problem.

## The built-in blocker

`src-tauri/src/inject/adblock.js` is injected into every content webview before
the page's own scripts. It never depends on an extension and works at three
levels:

1. **Player data.** `ytInitialPlayerResponse` and `ytInitialData` are written as
   object literals, not parsed JSON, so they are intercepted with accessor
   properties on `window`. `JSON.parse`, `Response.prototype.json` and the XHR
   `responseText` getter are wrapped as well. A recursive scrub removes ad keys
   (`adPlacements`, `playerAds`, `adSlots`, the ad-slot renderers) from any
   object, with a depth limit and a `WeakSet` for cycles. Removing the placement
   is what stops a video ad: with nothing in the player response, no ad break is
   ever scheduled.
2. **Network.** `fetch`, `XMLHttpRequest.open` and `navigator.sendBeacon` answer
   ad requests locally. Blocked fetches get a harmless `Response("{}")`; blocked
   XHRs are pointed at a `data:` URL so the XHR API still completes. Media from
   `googlevideo.com`, thumbnails from `ytimg.com` and `youtubei/v1/player` are
   explicitly excluded from the patterns.
3. **Page.** A constructed stylesheet is **adopted** rather than appended:

   ```js
   sheet = new CSSStyleSheet();
   sheet.replaceSync(css);
   document.adoptedStyleSheets = document.adoptedStyleSheets.concat(sheet);
   ```

   YouTube rebuilds its own head while it boots, and a `<style>` element added
   at `document_start` does not survive that. A throttled `MutationObserver`
   reapplies the sheet, and a 500 ms watchdog clicks `.ytp-ad-skip-button` or
   seeks to the end of an ad that still plays.

The player's own ad UI (`.ytp-ad-module`) is deliberately not hidden. If an ad
does slip through, its countdown and skip button have to stay usable.

`window.__ytpakeAdblock` exposes `stats`, the active filter version/source and
the pure `scrub()` / `isAdRequest()` helpers for DevTools probes.
`--no-adblock` turns the whole layer off without touching extensions.

## Runtime filter catalogue

Hard-coding selectors in the injected script would mean recompiling the app every
time YouTube changes its markup. Instead `filters/youtube.json` holds the keys,
URL patterns and selectors. `app/filters.rs` reads it at startup, validates its
shape and injects it as `window.__YTPAKE_ADBLOCK_FILTERS__` **before**
`adblock.js`. Lookup order, first valid file wins:

1. `--filters <path>` / `YTPAKE_FILTERS_FILE`
2. `<app data>/filters/youtube.json` (user update)
3. `<exe dir>/filters/youtube.json` (bundled resource)
4. the JSON compiled into the binary

An invalid file is skipped with a trace line; the blocker keeps running. A
malformed regex in the catalogue is caught when `adblock.js` compiles it.
`pnpm filters:check` rejects a catalogue that would block a player call, a
thumbnail or a media segment. Updating the filters is therefore replacing a
JSON file and restarting — see [filters/README.md](../filters/README.md).

## Window title and fullscreen

The title is rebuilt from the active tab's own title, with the profile suffix
appended when `--profileN` is in use, and re-applied on every activation and on
closing the active tab. The title callback ignores documents at `WARMUP_PATH`.

Fullscreen is detected by `fullscreenchange` in the page, forwarded through the
bridge, and the strip is resized to zero height while the content webview takes
the whole window. `Esc` restores both.

## Keyboard and pointer

`src-tauri/src/inject/tab_bridge.js` runs in every content webview before the
page's scripts. It intercepts the shortcuts in the table in the README,
`window.open` (turned into a tab rather than a native window), and middle clicks.

Middle click is not handled in the page: cancelling `mousedown` stops the
autoscroll but not the browser's own new-window request, so a tab opened there
would be a second one. The shell turns that request into a tab in
`on_new_window`, in the background.

In the strip, middle click closes a tab. That needs both halves: the `auxclick`
handler alone only ever fired for a synthetic click, because a middle press arms
autoscroll, the mouse is captured, and the `auxclick` never arrives. Cancelling
`mousedown` for button 1 keeps the release.

## Diagnostics

`tools/` holds the scripts used to verify all of this against the running
application over the DevTools protocol. They expect a built copy in
`dist/YouTube-portable`; see [tools/README.md](../tools/README.md).
`tools/adblock_probe.py` is the quick check for the blocker: with the app running
on `--remote-debugging-port=9222` it reports the active filter version and
source, the blocked-request/break/slot counters and any ad slot still visible.

The startup trace (`%TEMP%\ytpake-trace.log`) records the navigation policy,
the extension inventory and which filter catalogue was loaded, so a broken
extension folder or a malformed `youtube.json` is visible without a debugger.
