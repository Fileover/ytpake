// Injected into every content webview before the page's own scripts run.
// It turns the page into a tab-aware surface: keyboard shortcuts, middle
// clicks and window.open all end up in the native tab manager instead of
// opening a second window.
(function () {
  if (window.__pakeTabBridge) return;
  window.__pakeTabBridge = true;

  function invoke(command, args) {
    var bridge = window.__TAURI__ && window.__TAURI__.core;
    if (!bridge || typeof bridge.invoke !== "function") return null;
    try {
      return bridge.invoke(command, args || {});
    } catch (error) {
      return null;
    }
  }

  function absolute(href) {
    try {
      return new URL(href, window.location.href).href;
    } catch (error) {
      return null;
    }
  }

  function openInTab(href) {
    var url = absolute(href);
    if (!url || !/^https?:/i.test(url)) return false;
    invoke("tabs_new", { url: url });
    return true;
  }

  // window.open is how most sites spawn popups. Give those a tab instead of
  // a separate native window so the app stays a single window.
  var nativeOpen = window.open;
  window.open = function (url, target, features) {
    if (typeof url === "string" && url && !/^(about|javascript|data):/i.test(url)) {
      if (openInTab(url)) return null;
    }
    return nativeOpen.call(window, url, target, features);
  };

  // YouTube renders its cards inside shadow roots, so `event.target` is
  // retargeted to the custom element. composedPath() still walks the real
  // ancestor chain and finds the anchor.
  function findAnchor(event) {
    var path = typeof event.composedPath === "function" ? event.composedPath() : [];
    for (var i = 0; i < path.length; i++) {
      var node = path[i];
      if (node && node.tagName === "A" && typeof node.href === "string") return node;
    }
    var target = event.target;
    return target && target.closest ? target.closest("a[href]") : null;
  }

  // Middle-click is deliberately not handled here. Cancelling `mousedown`
  // stops the autoscroll but not the browser's own new-window request, which
  // WebView2 raises on mouse up, so a tab opened here would be a second one.
  // The shell already turns that request into a tab; see `on_new_window`.
  document.addEventListener(
    "mousedown",
    function (event) {
      if (event.button === 1) event.preventDefault();
    },
    true,
  );

  document.addEventListener(
    "click",
    function (event) {
      var anchor = findAnchor(event);
      if (!anchor) return;
      var wantsTab = event.ctrlKey || event.metaKey || anchor.target === "_blank";
      if (!wantsTab) return;
      if (openInTab(anchor.href)) {
        event.preventDefault();
        event.stopPropagation();
      }
    },
    true,
  );

  document.addEventListener(
    "keydown",
    function (event) {
      if (!(event.ctrlKey || event.metaKey) || event.altKey) return;
      var key = (event.key || "").toLowerCase();
      var handled = true;

      if (key === "t" && !event.shiftKey) {
        invoke("tabs_new", {});
      } else if (key === "w") {
        invoke("tabs_close_active", {});
      } else if (key === "r") {
        invoke("tabs_reload", {});
      } else if (key === "e" && event.shiftKey) {
        invoke("tabs_extension_menu", {});
      } else if (key === "tab") {
        invoke("tabs_cycle", { delta: event.shiftKey ? -1 : 1 });
      } else if (key === "9") {
        invoke("tabs_select", { index: -1 });
      } else if (/^[1-8]$/.test(key)) {
        invoke("tabs_select", { index: parseInt(key, 10) - 1 });
      } else {
        handled = false;
      }

      if (handled) {
        event.preventDefault();
        event.stopPropagation();
      }
    },
    true,
  );

  // Alt+Left / Alt+Right are the standard history shortcuts.
  document.addEventListener(
    "keydown",
    function (event) {
      if (!event.altKey || event.ctrlKey || event.metaKey) return;
      if (event.key === "ArrowLeft") {
        invoke("tabs_back", {});
        event.preventDefault();
      } else if (event.key === "ArrowRight") {
        invoke("tabs_forward", {});
        event.preventDefault();
      }
    },
    true,
  );

  // A fullscreen video has to cover the tab strip too, which is a separate
  // webview the page cannot reach; the shell resizes on this signal instead.
  document.addEventListener(
    "fullscreenchange",
    function () {
      invoke("tabs_fullscreen", { active: !!document.fullscreenElement });
    },
    true,
  );

  // Extension settings, opened by the gear button in the tab strip. WebView2
  // blocks chrome-extension:// as a top-level page, but the extension exposes
  // its menu as web-accessible for YouTube, so an iframe works here.
  window.__pakeOpenSettings = function (url) {
    if (!url) return;

    const existing = document.getElementById("pake-settings-overlay");
    if (existing) existing.remove();

    const overlay = document.createElement("div");
    overlay.id = "pake-settings-overlay";
    overlay.style.cssText = [
      "position:fixed", "inset:0", "z-index:2147483647",
      "background:rgba(0,0,0,0.72)", "display:flex",
      "align-items:center", "justify-content:center",
      "backdrop-filter:blur(2px)",
    ].join(";");

    const panel = document.createElement("div");
    panel.style.cssText = [
      "position:relative", "width:min(1120px,94vw)", "height:88vh",
      "background:#0f0f0f", "border:1px solid #2a2a2a",
      "border-radius:12px", "overflow:hidden",
      "box-shadow:0 24px 60px rgba(0,0,0,0.6)",
    ].join(";");

    const frame = document.createElement("iframe");
    frame.src = url;
    frame.style.cssText = "width:100%;height:100%;border:0;display:block;background:#0f0f0f";
    frame.setAttribute("allow", "clipboard-write");

    const close = document.createElement("button");
    close.textContent = "\u00d7";
    close.title = "Fechar (Esc)";
    close.style.cssText = [
      "position:absolute", "top:8px", "right:10px", "z-index:1",
      "width:30px", "height:30px", "border:0", "border-radius:8px",
      "background:rgba(255,255,255,0.08)", "color:#f1f1f1",
      "font:400 20px/1 system-ui,sans-serif", "cursor:pointer",
    ].join(";");
    close.addEventListener("click", () => overlay.remove());

    overlay.addEventListener("mousedown", (event) => {
      if (event.target === overlay) overlay.remove();
    });

    const onKey = (event) => {
      if (event.key === "Escape") {
        overlay.remove();
        document.removeEventListener("keydown", onKey, true);
      }
    };
    document.addEventListener("keydown", onKey, true);

    panel.append(frame, close);
    overlay.appendChild(panel);
    document.documentElement.appendChild(overlay);
  };

})();
