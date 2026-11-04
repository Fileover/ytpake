"""Check that the window caption follows the active tab.

Opens three tabs with different names, reads the caption, switches tabs and
reads it again, then closes the active one and reads it once more. The caption
comes from the OS, not from the app's own state, so this catches a title that
was only ever written when a page changed its own title.

Usage: python tools/title_check.py
"""
import json
import os
import subprocess
import sys
import time

sys.path.insert(0, "tools")
import round as harness
import shot

PORTABLE = harness.PORTABLE
TITLES = ["Rick Astley - Never Gonna Give You Up",
          "PSY - GANGNAM STYLE",
          "Luis Fonsi - Despacito"]


def main():
    with open(harness.SESSION, "w", encoding="utf-8") as handle:
        json.dump({
            "tabs": ["https://www.youtube.com/watch?v=dQw4w9WgXcQ",
                     "https://www.youtube.com/watch?v=9bZkp7q19f0",
                     "https://www.youtube.com/watch?v=kJQP7kiw5Fk"],
            "titles": TITLES,
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
    process = subprocess.Popen([os.path.join(PORTABLE, "YouTube.exe")], cwd=PORTABLE, env=env)

    try:
        time.sleep(4)
        shell = harness.strip()
        tabs = harness.state(shell)
        hwnd = shot.handle()

        def caption():
            return shot.title(hwnd)

        # Let the first page settle so its real title is in place.
        harness.settle(harness.wait_port(harness.CONTENT_FILE), 1)
        print("tab 0 active:", repr(caption()))

        for index in (1, 2, 0):
            shell.evaluate(
                "window.__TAURI__.core.invoke('tabs_activate',{id:%d})" % tabs[index]["id"]
            )
            time.sleep(2.5)
            print("tab %d active: %r" % (index, caption()))

        # Closing the active tab hands over to its neighbour.
        shell.evaluate(
            "window.__TAURI__.core.invoke('tabs_activate',{id:%d})" % tabs[2]["id"]
        )
        time.sleep(2)
        shell.evaluate("window.__TAURI__.core.invoke('tabs_close',{id:%d})" % tabs[2]["id"])
        for wait in (2, 4):
            time.sleep(wait)
            titles = [(tab["id"], tab["title"][:30]) for tab in harness.state(shell)]
            print("after closing tab 2 (+%ds): %r  strip %s" % (wait, caption(), titles))
    finally:
        process.terminate()


if __name__ == "__main__":
    main()
