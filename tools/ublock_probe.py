"""Look for a page-visible sign that the extensions ran in a tab.

Enhancer for YouTube marks the document element; uBlock Origin Lite leaves
nothing that obvious, so this dumps what a page can actually see: the document
class, every stylesheet the page loaded (an extension's CSS shows up as a
`chrome-extension://` URL) and the count of injected `<style>` elements.

Usage: python tools/ublock_probe.py
"""
import json
import os
import subprocess
import sys
import time

sys.path.insert(0, "tools")
import cdp
import round as harness

PROBE = """
(() => {
  const sheets = [...document.styleSheets]
    .map(s => s.href)
    .filter(href => href && href.startsWith('chrome-extension://'));
  // A cosmetic filter hides an element, it does not remove it: counting nodes
  // says nothing about what the user sees. Only boxes with an area count, and
  // the player carries `ad-showing` while an ad is really playing.
  const candidates = [...document.querySelectorAll(
    '.ad-showing, ytd-ad-slot-renderer, #masthead-ad, .ytp-ad-module, ytd-promoted-sparkles-web-renderer')];
  const visible = candidates.filter(el => {
    const box = el.getBoundingClientRect();
    return box.width > 0 && box.height > 0;
  });
  return JSON.stringify({
    cls: document.documentElement.className.slice(0, 40),
    sheets: sheets.length,
    styles: document.querySelectorAll('style').length,
    nodes: candidates.length,
    ads: visible.length,
    playerAd: !!document.querySelector('#movie_player.ad-showing')
  });
})()
"""


def sample(port, label, seconds=12):
    """Worst case per page over a window, not a single instant: an ad that
    appears and is hidden a moment later still counts as an ad the user saw."""
    worst = {}
    end = time.time() + seconds
    while time.time() < end:
        for target in harness.pages(port):
            url = target.get("url") or ""
            if "youtube.com" not in url or "robots" in url:
                continue
            session = cdp.Session(target["webSocketDebuggerUrl"])
            try:
                info = json.loads(session.evaluate(PROBE))
            finally:
                session.close()
            key = url.split("v=")[-1][:11]
            row = worst.setdefault(key, {"ads": 0, "playerAd": False, "styles": 0, "nodes": 0})
            row["ads"] = max(row["ads"], info["ads"])
            row["playerAd"] = row["playerAd"] or info["playerAd"]
            row["styles"] = max(row["styles"], info["styles"])
            row["nodes"] = max(row["nodes"], info["nodes"])
        time.sleep(1.0)
    for key, row in sorted(worst.items()):
        print("  %-9s %-12s %s" % (label, key, json.dumps(row)))


def main():
    with open(harness.SESSION, "w", encoding="utf-8") as handle:
        json.dump({
            "tabs": ["https://www.youtube.com/watch?v=dQw4w9WgXcQ"],
            "titles": ["Rick Astley"],
            "active": 0,
        }, handle, indent=2, ensure_ascii=False)

    subprocess.run(["taskkill", "/IM", "YouTube.exe", "/F"], capture_output=True)
    time.sleep(1.2)
    for path in (harness.STRIP_FILE, harness.CONTENT_FILE):
        try:
            os.remove(path)
        except OSError:
            pass
    env = dict(os.environ, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS="--remote-debugging-port=0")
    process = subprocess.Popen([os.path.join(harness.PORTABLE, "YouTube.exe")] + sys.argv[1:],
                               cwd=harness.PORTABLE, env=env)
    try:
        time.sleep(4)
        shell = harness.strip()
        harness.state(shell)
        port = harness.wait_port(harness.CONTENT_FILE)
        harness.settle(port, 1)
        sample(port, "first")
        # A tab opened later, with the extension host already up.
        shell.evaluate("window.__TAURI__.core.invoke('tabs_new',"
                       "{url:'https://www.youtube.com/watch?v=9bZkp7q19f0'})")
        time.sleep(14)
        sample(port, "later")
    finally:
        process.terminate()


if __name__ == "__main__":
    main()
