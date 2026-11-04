"""Dump how the strip is being laid out, for debugging layoutTabs().

Usage: python tools/layout_probe.py
"""
import json
import os
import subprocess
import sys
import time

sys.path.insert(0, "tools")
import round as harness

PROBE = """
(() => {
  const tabs = [...document.querySelectorAll('.tab')].map(t => {
    const s = getComputedStyle(t);
    return {
      inline: t.style.flexBasis,
      computed: s.flexBasis,
      flex: s.flex,
      minWidth: s.minWidth,
      width: s.width,
      rect: Math.round(t.getBoundingClientRect().width * 10) / 10,
      offset: t.offsetWidth,
      scroll: t.scrollWidth,
      label: t.querySelector('.label').textContent.slice(0, 18),
      parent: t.parentElement.className
    };
  });
  return JSON.stringify({
    hasFunction: typeof layoutTabs,
    tabs: tabs,
    bar: (() => {
      const b = document.querySelector('.bar');
      const s = getComputedStyle(b);
      return { client: b.clientWidth, gap: s.columnGap, pad: s.paddingLeft,
               kids: [...b.children].map(c => c.className + ':' + c.offsetWidth) };
    })()
  });
})()
"""


def main():
    with open(harness.SESSION, "w", encoding="utf-8") as handle:
        json.dump({
            "tabs": ["https://www.youtube.com/watch?v=dQw4w9WgXcQ",
                     "https://www.youtube.com/watch?v=9bZkp7q19f0",
                     "https://www.youtube.com/watch?v=kJQP7kiw5Fk",
                     "https://www.youtube.com/watch?v=HX80F1psI18"],
            "titles": ["Rick Astley", "PSY - Gangnam", "Luis Fonsi", "Ed Sheeran"],
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
        time.sleep(6)
        shell = harness.strip()
        tabs = harness.state(shell)
        shell.evaluate("window.__TAURI__.core.invoke('tabs_reload',{})")
        time.sleep(9)
        print(json.dumps(json.loads(shell.evaluate(PROBE)), indent=2, ensure_ascii=False))
    finally:
        process.terminate()


if __name__ == "__main__":
    main()
