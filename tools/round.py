"""End-to-end check of the tab behaviour this round changed.

Covers the three things that were asked for: the extension applying to every
tab (including restored ones, which now load on first use), a middle click
opening exactly one tab, and the strip showing the saved tabs immediately.

Usage: python tools/round.py
"""
import json
import os
import subprocess
import sys
import time
import urllib.request

sys.path.insert(0, "tools")
import cdp

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def find_app():
    """Locate the built application.

    `YTPAKE_APP` wins; otherwise the usual layouts are tried in order, so the
    same script works from the source tree and from a copy of it.
    """
    override = os.environ.get("YTPAKE_APP")
    if override:
        return override
    for candidate in (
        os.path.join(ROOT, "dist", "YouTube-portable"),
        os.path.join(ROOT, os.pardir, "dist", "YouTube-portable"),
        os.path.join(ROOT, "build", "YouTube-portable"),
    ):
        if os.path.isfile(os.path.join(candidate, "YouTube.exe")):
            return os.path.normpath(candidate)
    return os.path.normpath(os.path.join(ROOT, "dist", "YouTube-portable"))


PORTABLE = find_app()
SESSION = os.path.join(PORTABLE, "profile1", "session.json")
CONTENT_FILE = os.path.join(PORTABLE, "profile1", "webview", "EBWebView", "DevToolsActivePort")
STRIP_FILE = os.path.join(
    os.environ.get("LOCALAPPDATA", os.path.expanduser("~")),
    "com.pake.youtube.tabs",
    "EBWebView",
    "DevToolsActivePort",
)

CHECK = "document.documentElement.className.slice(0, 30)"
# The first matching link is often the one behind the player, where the click
# lands on the player's own overlay instead. Only offer a link whose centre
# really is on top, so the click reaches the anchor.
FIND = """
(() => {
  const links = [...document.querySelectorAll('a[href*="/watch?v="]')].filter(a => {
    const r = a.getBoundingClientRect();
    if (!(r.width > 60 && r.height > 40 && r.top > 40 && r.top < innerHeight - 60)) return false;
    const hit = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
    return !!(hit && a.contains(hit));
  });
  if (!links.length) return null;
  const a = links[0];
  const r = a.getBoundingClientRect();
  return {x: Math.round(r.left + r.width / 2), y: Math.round(r.top + r.height / 2),
          href: a.href};
})()
"""


def wait_port(path, timeout=45):
    """Wait for a DevToolsActivePort that was written by this launch.

    Both files survive the process that wrote them, so a stale one would point
    at a closed port. `main` deletes them before starting the app, and this
    waits for the browser to publish a fresh one.
    """
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            port = int(open(path).read().split()[0])
            pages(port)  # proves something is listening
            return port
        except Exception:
            time.sleep(0.5)
    raise SystemExit("no DevTools port at %s" % path)


def pages(port):
    with urllib.request.urlopen("http://127.0.0.1:%d/json" % port, timeout=5) as response:
        return [t for t in json.loads(response.read().decode()) if t["type"] == "page"]


def strip(timeout=20):
    # The port answers before tabs.html is a target, so the page itself is what
    # this waits for.
    port = wait_port(STRIP_FILE)
    deadline = time.time() + timeout
    while time.time() < deadline:
        for target in pages(port):
            if "tabs.html" in (target.get("url") or ""):
                return cdp.Session(target["webSocketDebuggerUrl"])
        time.sleep(0.5)
    return None


def state(shell, timeout=20):
    """Read the tab list, retrying while the IPC is still coming up.

    `evaluate` hands back `{"__error": ...}` when the invoke rejects, which is
    what happens if the strip is queried before Tauri's IPC is ready.
    """
    deadline = time.time() + timeout
    while True:
        value = shell.evaluate(
            "window.__TAURI__.core.invoke('tabs_state').then(s=>JSON.stringify(s.tabs))",
            await_promise=True,
        )
        if isinstance(value, str):
            return json.loads(value)
        if time.time() > deadline:
            raise SystemExit("tabs_state failed: %r" % (value,))
        time.sleep(1)


def applied(port):
    found = {}
    for target in pages(port):
        url = target.get("url") or ""
        if "youtube.com" not in url or "robots" in url:
            continue
        session = cdp.Session(target["webSocketDebuggerUrl"])
        found[url] = (session.evaluate(CHECK) or "")
        session.close()
    return found


def settle(port, expect, timeout=45):
    """Poll until `expect` watch pages report the extension, or time runs out.

    Fixed sleeps were guesswork: the extension host needs a few seconds after
    launch before its content scripts are registered, and how long depends on
    how much is loading. The count matters as much as the values — returning as
    soon as the pages already open look fine would skip the tab just opened.
    Only watch pages are asserted: the class under test belongs to the player,
    so the home page has nothing to show.
    """
    deadline = time.time() + timeout
    last = {}
    while time.time() < deadline:
        last = applied(port)
        watch = {url: value for url, value in last.items() if "watch?v=" in url}
        if len(watch) >= expect and all(watch.values()):
            return last
        time.sleep(2)
    return last


def main():
    # Start from a known two-tab session so the check is self-contained. Both
    # are watch pages: the class under test belongs to the player, so every tab
    # in the test has one to report on.
    session = {
        "tabs": ["https://www.youtube.com/watch?v=dQw4w9WgXcQ",
                 "https://www.youtube.com/watch?v=9bZkp7q19f0"],
        "titles": ["Rick Astley - Never Gonna Give You Up",
                   "PSY - GANGNAM STYLE(강남스타일)"],
        "active": 0,
    }
    with open(SESSION, "w", encoding="utf-8") as handle:
        json.dump(session, handle, indent=2, ensure_ascii=False)

    subprocess.run(["taskkill", "/IM", "YouTube.exe", "/F"], capture_output=True)
    time.sleep(1.2)
    # A port file left by the previous run points at a closed port.
    for path in (STRIP_FILE, CONTENT_FILE):
        try:
            os.remove(path)
        except OSError:
            pass
    env = dict(os.environ, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS="--remote-debugging-port=0")
    process = subprocess.Popen([os.path.join(PORTABLE, "YouTube.exe")], cwd=PORTABLE, env=env)

    try:
        time.sleep(4)
        shell = strip()
        tabs = state(shell)
        port = wait_port(CONTENT_FILE)
        print("1. restored strip: %d tabs, %d with a webview"
              % (len(tabs), len(pages(port))))
        for tab in tabs:
            print("     %d %-38s %s" % (tab["id"], tab["title"][:38], tab["url"][:52]))

        print("   extension on the visible tab:", settle(port, 1))

        # Open a second restored tab: it is built on demand.
        print("2. selecting tab", tabs[1]["id"])
        shell.evaluate("window.__TAURI__.core.invoke('tabs_activate',{id:%d})" % tabs[1]["id"])
        time.sleep(3)
        print("   extension there:", settle(port, 2))

        # Middle click on the active page. The link has to be on screen first,
        # so poll for it rather than trusting a fixed wait. The target must be
        # the active tab: clicking a background webview proves nothing, since
        # the request for a new window comes from the tab the user is on.
        active_url = tabs[1]["url"].split("v=")[1][:11]
        before = len(pages(port))
        session = None
        for target in pages(port):
            if active_url in (target.get("url") or ""):
                session = cdp.Session(target["webSocketDebuggerUrl"])
                break
        box = None
        deadline = time.time() + 30
        while session and time.time() < deadline:
            box = session.evaluate(FIND)
            if box:
                break
            time.sleep(2)
        if box:
            session.send("Page.enable")
            common = dict(x=box["x"], y=box["y"], button="middle", buttons=4,
                          clickCount=1, modifiers=0, pointerType="mouse")
            session.send("Input.dispatchMouseEvent", type="mousePressed", **common)
            time.sleep(0.05)
            session.send("Input.dispatchMouseEvent", type="mouseReleased", **common)
            # The tab is created in the background; give it time to appear.
            deadline = time.time() + 20
            while time.time() < deadline and len(pages(port)) == before:
                time.sleep(1)
            time.sleep(3)
            after = len(pages(port))
            active_after = shell.evaluate(
                "window.__TAURI__.core.invoke('tabs_state').then(s => String(s.active))",
                await_promise=True)
            print("3. middle click opened %d tab(s), active tab now %s"
                  % (after - before, active_after))
            print("   extension there:", settle(port, 3))
        else:
            print("3. no link to middle click")

        print("4. final strip:")
        for tab in state(shell):
            print("     %d %-38s %s" % (tab["id"], tab["title"][:38], tab["url"][:52]))
    finally:
        process.terminate()


if __name__ == "__main__":
    main()
