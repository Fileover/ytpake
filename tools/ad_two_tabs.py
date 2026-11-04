"""Survey ad slots in two tabs of one run.

Defaults to the home page first and a watch page second, which is the order
that matters: the home page is the one that kept its ads until a reload, and a
watch page opened later is the case that always worked.

    python tools/ad_two_tabs.py [first_url] [second_url]

Both pages are opened as the app would open them, so the warm-up hop is part of
what is being measured.
"""
import json
import os
import subprocess
import sys
import time

sys.path.insert(0, "tools")
import cdp
import round as harness
from ad_control import SURVEY

HOME = "https://www.youtube.com/"
WATCH = "https://www.youtube.com/watch?v=dQw4w9WgXcQ"


def survey(port, label):
    """Worst case per page over a few seconds, so a slot that fills late counts."""
    worst = {}
    for _ in range(8):
        for target in harness.pages(port):
            url = target.get("url") or ""
            if "youtube.com" not in url or "robots" in url or "warmup" in url:
                continue
            session = cdp.Session(target["webSocketDebuggerUrl"])
            try:
                info = json.loads(session.evaluate(SURVEY, await_promise=True))
            finally:
                session.close()
            key = url.split("v=")[-1][:11] if "watch" in url else url.rstrip("/")
            row = worst.setdefault(key, {"styles": 0, "filled": 0, "sample": []})
            if info["filled"] > row["filled"]:
                row["filled"] = info["filled"]
                row["sample"] = info["sample"][:2]
            row["styles"] = max(row["styles"], info["styles"])
        time.sleep(1.0)
    for key, row in sorted(worst.items()):
        print("  %-8s %-42s styles=%-3s visible ad slots=%s"
              % (label, key[-42:], row["styles"], row["filled"]))
        for item in row["sample"]:
            print("       ", item)


def main():
    first = sys.argv[1] if len(sys.argv) > 1 else HOME
    second = sys.argv[2] if len(sys.argv) > 2 else WATCH

    with open(harness.SESSION, "w", encoding="utf-8") as handle:
        json.dump({"tabs": [first], "titles": ["first"], "active": 0},
                  handle, indent=2, ensure_ascii=False)

    subprocess.run(["taskkill", "/IM", "YouTube.exe", "/F"], capture_output=True)
    time.sleep(1.2)
    for path in (harness.STRIP_FILE, harness.CONTENT_FILE):
        try:
            os.remove(path)
        except OSError:
            pass

    env = dict(os.environ, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS="--remote-debugging-port=0")
    process = subprocess.Popen([os.path.join(harness.PORTABLE, "YouTube.exe")],
                               cwd=harness.PORTABLE, env=env)
    try:
        time.sleep(4)
        shell = harness.strip()
        harness.state(shell)
        port = harness.wait_port(harness.CONTENT_FILE)
        harness.settle(port, 1)
        time.sleep(6)
        survey(port, "tab 1")

        shell.evaluate("window.__TAURI__.core.invoke('tabs_new',{url:'%s'})" % second)
        time.sleep(16)
        survey(port, "tab 2")
    finally:
        process.terminate()


if __name__ == "__main__":
    main()
