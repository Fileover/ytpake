# Diagnostics

Scripts that drive the running application over the DevTools protocol and report
what it actually does, rather than what it looks like it does. They were written
while building the tab system, and every claim in
[docs/architecture.md](../docs/architecture.md) was measured with them.

## Setup

```bash
pip install websocket-client pillow
```

The application has to be built first, and the scripts expect it at
`dist/YouTube-portable/YouTube.exe` (or wherever `YTPAKE_APP` points):

```bash
YTPAKE_APP=/path/to/YouTube-portable python tools/round.py
```

They launch the app themselves, with
`WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=0`, and read the
port WebView2 writes to `DevToolsActivePort`. Two are involved: one for the tab
strip and one for the page, and the scripts know where each lives.

> The app must not be running when a script starts: they kill it, delete the stale
> port files and launch their own copy. Anything unsaved in a running window is
> lost.

## The scripts

| Script | What it answers |
| --- | --- |
| `round.py` | End-to-end: restored tabs, the extension in every tab, a middle click opening exactly one tab in the background. This is the one to run after a change. |
| `stability.py` | Do tab widths and nodes stay put while a page loads? Reports distinct widths per tab and how many times the strip was rebuilt. |
| `ad_control.py` | What the page looks like with and without the extensions: `<style>` count, ad slots holding a real ad. Run it twice, once with `off`. |
| `ad_two_tabs.py` | The same survey across two tabs — the home page first, a watch page second. |
| `ublock_probe.py` | First load versus a later load: is the extension present, are the ads gone. |
| `warmup_tune.py` | How short the warm-up wait can be before a tab loses the extension. Takes the delay in milliseconds and the number of tabs. |
| `title_check.py` | The window caption, read from the operating system, after each tab switch and after closing. |
| `layout_probe.py`, `click_tab.py` | Tab geometry, and clicking a tab by position. |
| `cdp.py` | The protocol helper: WebSocket session, `Runtime.evaluate`, input events. |
| `shot.py` | Screenshot of a window by handle. |

## Notes on the environment

Two things cost an hour each and are worth knowing before writing another script:

- **The shell has `HTTP_PROXY` set**, and both `urllib` and `websocket` send
  `127.0.0.1` through it, where the proxy answers 502. `cdp.py` clears `no_proxy`
  and installs an empty proxy handler.
- **A `DevToolsActivePort` file left by a previous run points at a closed port.**
  Every script deletes both files before launching and then polls for a port that
  answers.
