"""Measure what a shorter warm-up costs a tab opened after the first one.

`PAKE_WARMUP_DELAY_MS` overrides the built-in delays, so a value can be tried
without rebuilding. The first tab is opened from the session file and ignored:
the point here is tabs opened while the extension host is already running, and
the first tab of a run is the one that needs the long warm-up.

For each tab this records how long the strip shows it as loading (the app's own
idea of "not ready yet") and, at the end, whether the content scripts actually
ran in it.

Usage: python tools/warmup_tune.py [delay_ms] [tabs]
"""
import json
import os
import subprocess
import sys
import time

sys.path.insert(0, "tools")
import cdp
import round as harness

VIDEOS = [
    "https://www.youtube.com/watch?v=9bZkp7q19f0",
    "https://www.youtube.com/watch?v=9gh7Lt7tdj8",
    "https://www.youtube.com/watch?v=kJQP7kiw5Fk",
    "https://www.youtube.com/watch?v=3JZ_D3ELwOQ",
]
# Wider than TAB_SPACING, so each tab is created immediately instead of
# queueing behind the previous one — the wait is what is being measured.
GAP = 6.0


def extension_state(port):
    """Map url -> document class, for pages that have loaded something."""
    found = {}
    for target in harness.pages(port):
        url = target.get("url") or ""
        if "youtube.com" not in url or "robots" in url:
            continue
        session = cdp.Session(target["webSocketDebuggerUrl"])
        try:
            raw = session.evaluate(
                "JSON.stringify({cls: document.documentElement.className.slice(0,34),"
                " ready: document.readyState})"
            )
            found[url] = json.loads(raw)
        finally:
            session.close()
    return found


def main():
    delay = sys.argv[1] if len(sys.argv) > 1 else "1000"
    count = int(sys.argv[2]) if len(sys.argv) > 2 else 3

    with open(harness.SESSION, "w", encoding="utf-8") as handle:
        json.dump({"tabs": [VIDEOS[0]], "titles": ["first"], "active": 0},
                  handle, indent=2, ensure_ascii=False)

    subprocess.run(["taskkill", "/IM", "YouTube.exe", "/F"], capture_output=True)
    time.sleep(1.2)
    for path in (harness.STRIP_FILE, harness.CONTENT_FILE):
        try:
            os.remove(path)
        except OSError:
            pass

    env = dict(os.environ,
               WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS="--remote-debugging-port=0",
               PAKE_WARMUP_DELAY_MS=delay)
    process = subprocess.Popen([os.path.join(harness.PORTABLE, "YouTube.exe")],
                               cwd=harness.PORTABLE, env=env)
    try:
        time.sleep(4)
        shell = harness.strip()
        state = harness.state(shell)
        port = harness.wait_port(harness.CONTENT_FILE)
        print("delay %sms, waiting for the first tab" % delay)
        time.sleep(12)

        for index in range(count):
            url = VIDEOS[index + 1]
            started = time.time()
            shell.evaluate("window.__TAURI__.core.invoke('tabs_new',{url:'%s'})" % url)
            last = started
            tabs = []
            while time.time() - started < 45:
                time.sleep(0.25)
                tabs = harness.state(shell) or []
                if tabs and not tabs[-1].get("loading"):
                    last = time.time()
                    break
            print("  tab %d: strip settled after %.1fs  (%s)"
                  % (index + 2, last - started, (tabs[-1] if tabs else {}).get("title", "")[:40]))
            time.sleep(GAP)

        time.sleep(3)
        found = extension_state(port)
        print("  pages with content scripts:")
        for url, info in sorted(found.items()):
            mark = "OK  " if "efyt" in info["cls"] else "MISS"
            print("   ", mark, url.split("v=")[-1], info)
        missing = [u for u, i in found.items() if "efyt" not in i["cls"]]
        print("  result: %d/%d pages have the extension"
              % (len(found) - len(missing), len(found)))
    finally:
        process.terminate()


if __name__ == "__main__":
    main()
