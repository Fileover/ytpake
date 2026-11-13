"""Report the built-in blocker's live state in every YouTube tab.

Start the app with
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222
then run: python tools/adblock_probe.py

This answers "is the blocker running, which filter version is active, and what
has it actually blocked?" without guessing from a screenshot.
"""
import json
import sys

sys.path.insert(0, "tools")
import cdp

PROBE = """
(() => {
  const a = window.__ytpakeAdblock;
  if (!a) return JSON.stringify({present: false});
  const sheets = [...document.styleSheets]
    .map(s => s.href)
    .filter(href => href && href.startsWith('chrome-extension://'));
  const visible = [...document.querySelectorAll(
    'ytd-ad-slot-renderer, #masthead-ad, #player-ads, ytd-banner-promo-renderer')]
    .filter(el => {
      const box = el.getBoundingClientRect();
      return box.width > 0 && box.height > 0;
    });
  return JSON.stringify({
    present: true,
    enabled: a.enabled,
    filters: a.filters,
    stats: a.stats,
    visibleAdSlots: visible.length,
    playerAd: !!document.querySelector('#movie_player.ad-showing'),
    extensionSheets: sheets.length
  });
})()
"""


def main():
    found = 0
    for target in cdp.targets():
        url = target.get("url") or ""
        if target.get("type") != "page" or "youtube.com" not in url:
            continue
        session = cdp.Session(target["webSocketDebuggerUrl"])
        try:
            value = session.evaluate(PROBE)
        finally:
            session.close()
        if not value:
            continue
        found += 1
        print(url[:100])
        print(json.dumps(json.loads(value), indent=2, ensure_ascii=False))
    if not found:
        print("No YouTube page found. Is the app running with remote debugging on port 9222?")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
