"""Measure whether the tab strip keeps resizing itself.

Samples the width of every tab repeatedly while a tab loads, and reports how
many distinct widths each tab showed. A stable strip reports one value per tab.
It also tags each tab node and checks the tag survives, which tells a rebuilt
strip apart from one that is updated in place.

Usage: python tools/stability.py [samples]
"""
import json
import os
import subprocess
import sys
import time

sys.path.insert(0, "tools")
import cdp
import round as harness

PORTABLE = harness.PORTABLE
SAMPLES = int(sys.argv[1]) if len(sys.argv) > 1 else 40

SAMPLE = """
(() => {
  const tabs = [...document.querySelectorAll('.tab')].map(t => ({
    id: t.dataset.id,
    w: Math.round(t.getBoundingClientRect().width * 10) / 10,
    kept: t.__probe === 1
  }));
  const box = selector => {
    const node = document.querySelector(selector);
    if (!node) return null;
    const rect = node.getBoundingClientRect();
    return { w: Math.round(rect.width), shown: getComputedStyle(node).display !== 'none' };
  };
  return JSON.stringify({
    tabs: tabs,
    strip: box('.tabs'),
    plus: box('.new-tab'),
    extensions: box('#extensions'),
    spacer: box('.spacer'),
    header: box('.header'),
    basis: getComputedStyle(document.querySelector('.tab')).flexBasis,
    scroll: document.querySelector('.tabs').scrollWidth,
    client: document.querySelector('.tabs').clientWidth,
    window: innerWidth
  });
})()
"""

TAG = "document.querySelectorAll('.tab').forEach(t => { t.__probe = 1; })"


def main():
    with open(harness.SESSION, "w", encoding="utf-8") as handle:
        json.dump({
            "tabs": ["https://www.youtube.com/watch?v=dQw4w9WgXcQ",
                     "https://www.youtube.com/watch?v=9bZkp7q19f0",
                     "https://www.youtube.com/watch?v=kJQP7kiw5Fk",
                     "https://www.youtube.com/watch?v=HX80F1psI18"],
            "titles": ["Rick Astley - Never Gonna Give You Up",
                       "PSY - GANGNAM STYLE(강남스타일)",
                       "Luis Fonsi - Despacito",
                       "Ed Sheeran - Shape of You"],
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
        shell.evaluate(TAG)

        # Selecting a tab that was never built makes it load, which is when the
        # strip used to redraw the most.
        shell.evaluate("window.__TAURI__.core.invoke('tabs_activate',{id:%d})" % tabs[1]["id"])

        seen = {}
        rebuilt = 0
        trace = []
        for index in range(SAMPLES):
            sample = json.loads(shell.evaluate(SAMPLE))
            rows = sample["tabs"]
            for row in rows:
                seen.setdefault(row["id"], set()).add(row["w"])
            if rows and not all(row["kept"] for row in rows):
                rebuilt += 1
            trace.append("%2d tab %.1f strip %s/%s plus %s spacer %s ext %s basis %s win %d"
                         % (index,
                            rows[0]["w"] if rows else 0,
                            sample["client"], sample["scroll"],
                            sample["plus"] and sample["plus"]["w"],
                            sample["spacer"] and sample["spacer"]["w"],
                            (sample["extensions"] or {}).get("shown"),
                            sample["basis"],
                            sample["window"]))
            time.sleep(0.25)

        print("samples: %d" % SAMPLES)
        for tab in tabs:
            widths = sorted(seen.get(str(tab["id"]), []))
            print("  tab %s  %-34s widths %s"
                  % (tab["id"], tab["title"][:34],
                     ", ".join("%.1f" % value for value in widths)))
        print("samples where a tab node had been rebuilt: %d" % rebuilt)

        # Show the samples around the first change, which is where the cause is.
        first = next((i for i in range(1, len(trace))
                      if trace[i].split("tab ")[1][:6] != trace[i - 1].split("tab ")[1][:6]), None)
        if first is None:
            print("no width change during the run")
        else:
            print("first change at sample %d:" % first)
            for line in trace[max(0, first - 2):first + 3]:
                print("   ", line)
    finally:
        process.terminate()


if __name__ == "__main__":
    main()
