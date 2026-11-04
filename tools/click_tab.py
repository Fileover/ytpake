"""Click a tab in the strip with a real mouse event and report the result.

The strip reuses its tab elements instead of rebuilding them, and the click
handlers read the id from the element, so this drives the actual path a user
takes rather than the IPC call the other tools use.

Usage: python tools/click_tab.py [tab-index]
"""
import json
import os
import subprocess
import sys
import time

sys.path.insert(0, "tools")
import cdp
import round as harness

INDEX = int(sys.argv[1]) if len(sys.argv) > 1 else 1


def main():
    with open(harness.SESSION, "w", encoding="utf-8") as handle:
        json.dump({
            "tabs": ["https://www.youtube.com/watch?v=dQw4w9WgXcQ",
                     "https://www.youtube.com/watch?v=9bZkp7q19f0",
                     "https://www.youtube.com/watch?v=kJQP7kiw5Fk"],
            "titles": ["Rick Astley", "PSY - Gangnam", "Luis Fonsi"],
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
    process = subprocess.Popen([os.path.join(harness.PORTABLE, "YouTube.exe")],
                               cwd=harness.PORTABLE, env=env)
    try:
        time.sleep(5)
        shell = harness.strip()
        tabs = harness.state(shell)
        print("before: active", shell.evaluate(
            "window.__TAURI__.core.invoke('tabs_state').then(s => String(s.active))",
            await_promise=True))

        box = json.loads(shell.evaluate(
            "(() => { const t = document.querySelectorAll('.tab')[%d];"
            " const b = t.getBoundingClientRect();"
            " return JSON.stringify([b.left + b.width / 2, b.top + b.height / 2]); })()" % INDEX
        ))
        common = dict(x=box[0], y=box[1], button="left", buttons=1,
                      clickCount=1, modifiers=0, pointerType="mouse")
        shell.send("Input.dispatchMouseEvent", type="mousePressed", **common)
        time.sleep(0.05)
        shell.send("Input.dispatchMouseEvent", type="mouseReleased", **common)
        time.sleep(2)

        active = shell.evaluate(
            "window.__TAURI__.core.invoke('tabs_state').then(s => String(s.active))",
            await_promise=True)
        titles = [(tab["id"], tab["title"][:24]) for tab in harness.state(shell)]
        print("after clicking tab %d: active %s  strip %s" % (INDEX, active, titles))
    finally:
        process.terminate()


if __name__ == "__main__":
    main()
