"""Minimal Chrome DevTools Protocol client for the WebView2 instance.

The app is launched with WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222,
which lets us inspect the real page instead of guessing from screenshots.
"""
import json
import os
import sys
import time
import urllib.request

import websocket

BASE = "http://127.0.0.1:9222"

# This shell can carry HTTP_PROXY/HTTPS_PROXY, and both urllib and the
# websocket client would send the DevTools requests for 127.0.0.1 through that
# proxy, which answers 502. Every tool imports this module, so the bypass is
# installed once, here.
os.environ["no_proxy"] = os.environ["NO_PROXY"] = "127.0.0.1,localhost,::1"
urllib.request.install_opener(urllib.request.build_opener(urllib.request.ProxyHandler({})))


def _get(path):
    with urllib.request.urlopen(BASE + path, timeout=5) as response:
        return json.loads(response.read().decode("utf-8"))


class Session:
    def __init__(self, ws_url):
        self.ws = websocket.create_connection(ws_url, timeout=20)
        self.seq = 0

    def send(self, method, **params):
        self.seq += 1
        self.ws.send(json.dumps({"id": self.seq, "method": method, "params": params}))
        while True:
            message = json.loads(self.ws.recv())
            if message.get("id") == self.seq:
                if "error" in message:
                    raise RuntimeError(message["error"])
                return message.get("result", {})

    def evaluate(self, expression, await_promise=False):
        result = self.send(
            "Runtime.evaluate",
            expression=expression,
            returnByValue=True,
            awaitPromise=await_promise,
            userGesture=True,
        )
        if result.get("exceptionDetails"):
            details = result["exceptionDetails"]
            text = details.get("exception", {}).get("description") or details.get("text")
            return {"__error": text}
        return result.get("result", {}).get("value")

    def close(self):
        try:
            self.ws.close()
        except Exception:
            pass


def targets():
    """All targets, including child webviews that /json omits."""
    browser = Session(_get("/json/version")["webSocketDebuggerUrl"])
    try:
        return browser.send("Target.getTargets")["targetInfos"]
    finally:
        browser.close()


def attach(target_id):
    browser = Session(_get("/json/version")["webSocketDebuggerUrl"])
    try:
        reply = browser.send("Target.attachToTarget", targetId=target_id, flatten=True)
        return reply["sessionId"], browser
    finally:
        pass


def page_session(match="youtube.com"):
    for info in targets():
        if info.get("type") == "page" and match in (info.get("url") or ""):
            return Session(info["webSocketDebuggerUrl"])
    return None


if __name__ == "__main__":
    for info in targets():
        print(
            "%s | %-40s | %s"
            % (info.get("type"), (info.get("title") or "")[:40], (info.get("url") or "")[:100])
        )
