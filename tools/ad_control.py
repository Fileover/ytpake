"""Compare what a page looks like with and without the extensions loaded.

The watch page only serves an ad when it feels like it, so "0 ads" there proves
nothing on its own. The home page is the deterministic one: its masthead banner
and promoted shelf are always filled. Run it twice —

    python tools/ad_control.py            # with extensions
    python tools/ad_control.py off        # with --no-extensions

— and the difference is what the blocker is doing.
"""
import json
import os
import subprocess
import sys
import time

sys.path.insert(0, "tools")
import cdp
import round as harness

SURVEY = """
(() => {
  const selectors = ['#masthead-ad', 'ytd-ad-slot-renderer', 'ytd-display-ad-renderer',
                     'ytd-promoted-sparkles-web-renderer', 'ytd-in-feed-ad-layout-renderer',
                     'ytd-banner-promo-renderer', '#player-ads'];
  // A slot keeps its size even when the blocker emptied it, so the box says
  // nothing. What matters is whether there is an ad *inside*: an iframe from an
  // ad host, an image, or the "Ad" badge YouTube puts on a paid result.
  const filled = [];
  for (const selector of selectors) {
    for (const el of document.querySelectorAll(selector)) {
      const box = el.getBoundingClientRect();
      if (!(box.width > 0 && box.height > 0) || getComputedStyle(el).display === 'none') continue;
      const iframes = [...el.querySelectorAll('iframe')].map(f => f.src || '(inline)');
      const images = el.querySelectorAll('img').length;
      const text = (el.innerText || '').replace(/\\s+/g, ' ').trim().slice(0, 50);
      const adBadge = !!el.querySelector(
        '.badge-style-type-ad, ytd-badge-supported-renderer:has(.badge-style-type-ad)');
      if (iframes.length || images || adBadge) {
        filled.push({sel: selector, iframes: iframes.slice(0, 2), images, adBadge, text});
      }
    }
  }
  // A `fetch` here would be a request of type xmlhttprequest, and the rules
  // that matter are written for scripts — asking that way reported "reached"
  // for an ad script uBlock does block. To test blocking, import the script as
  // a module instead; see the note in LEIA-ME.txt.
  return JSON.stringify({
    cls: document.documentElement.className.slice(0, 40),
    styles: document.querySelectorAll('style').length,
    filled: filled.length,
    sample: filled.slice(0, 3)
  });
})()
"""


def main():
    mode = sys.argv[1] if len(sys.argv) > 1 else "on"

    with open(harness.SESSION, "w", encoding="utf-8") as handle:
        json.dump({"tabs": ["https://www.youtube.com/"], "titles": ["YouTube"],
                   "active": 0}, handle, indent=2, ensure_ascii=False)

    subprocess.run(["taskkill", "/IM", "YouTube.exe", "/F"], capture_output=True)
    time.sleep(1.2)
    for path in (harness.STRIP_FILE, harness.CONTENT_FILE):
        try:
            os.remove(path)
        except OSError:
            pass

    extra = ["--no-extensions"] if mode == "off" else []
    env = dict(os.environ, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS="--remote-debugging-port=0")
    process = subprocess.Popen([os.path.join(harness.PORTABLE, "YouTube.exe")] + extra,
                               cwd=harness.PORTABLE, env=env)
    try:
        time.sleep(4)
        harness.state(harness.strip())
        port = harness.wait_port(harness.CONTENT_FILE)
        time.sleep(14)

        best = None
        for target in harness.pages(port):
            url = target.get("url") or ""
            if "youtube.com" not in url or "robots" in url:
                continue
            session = cdp.Session(target["webSocketDebuggerUrl"])
            try:
                info = json.loads(session.evaluate(SURVEY, await_promise=True))
            finally:
                session.close()
            if best is None or info["filled"] > best["filled"]:
                best = info

        print("extensions:", "on" if mode == "on" else "off")
        print("  document class:", best["cls"] or "(none)")
        print("  <style> injected:", best["styles"])
        print("  slots holding an ad:", best["filled"])
        for item in best["sample"]:
            print("   ", item)
    finally:
        process.terminate()


if __name__ == "__main__":
    main()
