"""Locate the app window and take exact client-area screenshots.

The window is resized by the window-state plugin between runs, so every helper
here resolves the geometry at call time instead of hardcoding coordinates.
"""

import ctypes
import sys
import time
from ctypes import wintypes

from PIL import ImageGrab

u = ctypes.windll.user32

WNDPROC = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)


def windows(cls="Tauri Window"):
    found = []

    def callback(hwnd, _):
        if not u.IsWindowVisible(hwnd):
            return True
        name = ctypes.create_unicode_buffer(256)
        u.GetClassNameW(hwnd, name, 256)
        if name.value == cls:
            found.append(hwnd)
        return True

    u.EnumWindows(WNDPROC(callback), 0)
    return found


def handle(cls="Tauri Window"):
    found = windows(cls)
    if not found:
        raise SystemExit("app window not found")
    return found[0]


def client_rect(hwnd):
    rect = wintypes.RECT()
    u.GetClientRect(hwnd, ctypes.byref(rect))
    origin = wintypes.POINT(0, 0)
    u.ClientToScreen(hwnd, ctypes.byref(origin))
    return (
        origin.x,
        origin.y,
        origin.x + rect.right,
        origin.y + rect.bottom,
    )


def title(hwnd):
    length = u.GetWindowTextLengthW(hwnd)
    buf = ctypes.create_unicode_buffer(length + 1)
    u.GetWindowTextW(hwnd, buf, length + 1)
    return buf.value


def focus(hwnd, settle=0.9):
    u.ShowWindow(hwnd, 9)  # SW_RESTORE
    u.SetForegroundWindow(hwnd)
    time.sleep(settle)


def shot(path, hwnd=None):
    hwnd = hwnd or handle()
    focus(hwnd)
    img = ImageGrab.grab()
    left, top, right, bottom = client_rect(hwnd)
    img.crop((left, top, right, bottom)).save(path)
    return (left, top, right - left, bottom - top)


def click(hwnd, x, y, settle=0.0):
    """Click at a client-relative position."""
    left, top, _, _ = client_rect(hwnd)
    ctypes.windll.user32.SetCursorPos(left + x, top + y)
    time.sleep(0.05)
    ctypes.windll.user32.mouse_event(0x0002, 0, 0, 0, 0)  # left down
    ctypes.windll.user32.mouse_event(0x0004, 0, 0, 0, 0)  # left up
    if settle:
        time.sleep(settle)


if __name__ == "__main__":
    hwnd = handle()
    print("hwnd", hwnd, "title", repr(title(hwnd)), "client", client_rect(hwnd))
    if len(sys.argv) > 1:
        print("saved", shot(sys.argv[1], hwnd))
