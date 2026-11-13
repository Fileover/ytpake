// Built-in YouTube ad blocker. Injected into every content webview before the
// page's own scripts run (WebviewBuilder::initialization_script).
//
// Why this exists: WebView2 loads uBlock Origin Lite's network rules but does
// not apply its cosmetic filters, and an unpacked extension is optional —
// a fresh install has no extensions folder at all. Depending on uBO Lite alone
// therefore leaves a build with visible ads. This blocker is always present.
//
// The filter catalogue is NOT hard-coded here. Rust injects
// window.__YTPAKE_ADBLOCK_FILTERS__ from filters/youtube.json (or a user
// override) before this script, so YouTube changing its markup means dropping
// a new JSON next to the executable — no rebuild. The small FALLBACK below
// only covers the case where that injection never happened.
//
// It works at three levels:
//   1. Player data: ad placements are removed from ytInitialPlayerResponse and
//      from every Innertube JSON response, so no ad break is ever scheduled.
//   2. Network: known ad endpoints are answered locally before any request
//      leaves the webview.
//   3. Page: ad slots are hidden with an adopted stylesheet, and an ad that
//      still slips through is skipped in the player.
//
// The player's own ad module is not hidden: if an ad ever plays, its countdown
// and skip button have to stay usable.
(function () {
  "use strict";
  if (window.__ytpakeAdblock) return;

  var host = (location.hostname || "").toLowerCase();
  var isYouTube =
    host === "youtube.com" ||
    host.endsWith(".youtube.com") ||
    host === "youtube-nocookie.com" ||
    host.endsWith(".youtube-nocookie.com");
  if (!isYouTube) return;

  var enabled = window.__YTPAKE_ADBLOCK_ENABLED__ !== false;
  var stats = {
    requests: 0,
    responses: 0,
    slots: 0,
    breaks: 0,
    skips: 0,
    css: 0,
  };

  // Emergency-only fallback. The canonical lists live in filters/youtube.json;
  // this is enough to keep working if the injection step failed or the app was
  // built without the filter resource.
  var FALLBACK = {
    version: "emergency-fallback",
    adKeys: [
      "adPlacements",
      "playerAds",
      "adSlots",
      "adBreak",
      "adBreaks",
      "adSlotRenderer",
      "inFeedAdLayoutRenderer",
      "promotedVideoRenderer",
      "displayAdRenderer",
      "bannerPromoRenderer",
      "companionSlotRenderer",
      "actionCompanionAdRenderer",
      "searchPyvRenderer",
    ],
    breakKeys: ["adPlacements", "playerAds", "adSlots", "adBreak", "adBreaks"],
    urlPatterns: [
      "doubleclick\\.net",
      "googlesyndication\\.com",
      "googleadservices\\.com",
      "adservice\\.google\\.",
      "/pagead[/?]",
      "/api/ads[/?]",
      "/api/stats/ads",
      "/get_ads?\\b",
      "get_midroll",
      "ad_break",
      "[?&]adformat=",
      "[?&]ad_type=",
      "/ads(\\?|/|$)",
    ],
    selectors: [
      "ytd-ad-slot-renderer",
      "ytd-display-ad-renderer",
      "ytd-in-feed-ad-layout-renderer",
      "ytd-promoted-video-renderer",
      "ytd-banner-promo-renderer",
      "#masthead-ad",
      "#player-ads",
      "#ad-text",
      ".ytp-ad-text",
      ".ytp-ad-overlay-container",
    ],
    containerSelectors: ["ytd-rich-item-renderer:has(ytd-ad-slot-renderer)"],
  };

  function stringList(value) {
    if (!Array.isArray(value)) return [];
    var result = [];
    for (var i = 0; i < value.length; i++) {
      if (typeof value[i] === "string" && value[i].trim()) {
        result.push(value[i].trim());
      }
    }
    return result;
  }

  function keyMap(keys) {
    var map = {};
    for (var i = 0; i < keys.length; i++) map[keys[i]] = 1;
    return map;
  }

  function normalize(raw) {
    if (!raw || typeof raw !== "object") return null;
    var adKeys = stringList(raw.adKeys);
    var selectors = stringList(raw.selectors);
    var containers = stringList(raw.containerSelectors);
    var rules = [];
    var patterns = stringList(raw.urlPatterns);
    for (var i = 0; i < patterns.length; i++) {
      try {
        rules.push(new RegExp(patterns[i], "i"));
      } catch (error) {}
    }
    if (!adKeys.length || !selectors.length || !rules.length) return null;

    var textRe;
    try {
      textRe = new RegExp(adKeys.join("|"));
    } catch (error) {
      textRe = /adPlacements|playerAds|adSlots/;
    }
    var hide = "{display:none!important;visibility:hidden!important}";
    return {
      version: String(raw.version || "custom"),
      adKeys: keyMap(adKeys),
      breakKeys: keyMap(stringList(raw.breakKeys)),
      urlRules: rules,
      selectors: selectors,
      containers: containers,
      textRe: textRe,
      css:
        selectors.join(",") +
        hide +
        (containers.length ? "\n" + containers.join(",") + hide : ""),
    };
  }

  var supplied = normalize(window.__YTPAKE_ADBLOCK_FILTERS__);
  var filters = supplied || normalize(FALLBACK);
  var filterSource = supplied ? "file" : "fallback";

  var MAX_DEPTH = 30;
  var hasOwn = Object.prototype.hasOwnProperty;

  function isAdRequest(url) {
    var text = String(url || "");
    for (var i = 0; i < filters.urlRules.length; i++) {
      if (filters.urlRules[i].test(text)) return true;
    }
    return false;
  }

  function scrub(value, depth, seen) {
    if (!value || typeof value !== "object" || (depth || 0) > MAX_DEPTH)
      return value;
    seen = seen || new WeakSet();
    if (seen.has(value)) return value;
    seen.add(value);

    if (Array.isArray(value)) {
      for (var index = 0; index < value.length; index++) {
        var item = value[index];
        if (item && typeof item === "object")
          scrub(item, (depth || 0) + 1, seen);
      }
      return value;
    }

    var keys = Object.keys(value);
    for (var i = 0; i < keys.length; i++) {
      var key = keys[i];
      if (hasOwn.call(filters.adKeys, key)) {
        try {
          delete value[key];
        } catch (error) {
          try {
            value[key] = null;
          } catch (ignored) {}
        }
        if (hasOwn.call(filters.breakKeys, key)) stats.breaks++;
        else stats.slots++;
        continue;
      }
      var child;
      try {
        child = value[key];
      } catch (error) {
        continue;
      }
      if (child && typeof child === "object")
        scrub(child, (depth || 0) + 1, seen);
    }
    return value;
  }

  function scrubText(text) {
    if (typeof text !== "string" || !filters.textRe.test(text)) return text;
    try {
      var parsed = JSON.parse(text);
      return JSON.stringify(scrub(parsed));
    } catch (error) {
      return text;
    }
  }

  // Exposed for the diagnostics scripts in tools/ and for unit tests: these
  // helpers are pure enough to call from a DevTools console.
  window.__ytpakeAdblock = {
    enabled: enabled,
    stats: stats,
    filters: {
      version: filters.version,
      source: filterSource,
      selectors: filters.selectors.length + filters.containers.length,
      urlRules: filters.urlRules.length,
    },
    selectors: filters.selectors.concat(filters.containers),
    css: filters.css,
    scrub: scrub,
    scrubText: scrubText,
    isAdRequest: isAdRequest,
  };
  // When the blocker is off, no global is patched at all.
  if (!enabled) return;

  // --- Level 1: player data -------------------------------------------------

  // YouTube writes these two globals as plain object literals, so JSON.parse
  // never sees them. An accessor property intercepts the assignment instead.
  function guardGlobal(name) {
    var current;
    try {
      Object.defineProperty(window, name, {
        configurable: true,
        enumerable: true,
        get: function () {
          return current;
        },
        set: function (value) {
          current = scrub(value);
        },
      });
    } catch (error) {
      // A locked-down document loses this layer; the other two still apply.
    }
  }
  guardGlobal("ytInitialPlayerResponse");
  guardGlobal("ytInitialData");

  var nativeParse = JSON.parse;
  JSON.parse = function (text) {
    var value = nativeParse.apply(this, arguments);
    if (typeof text === "string" && filters.textRe.test(text)) {
      stats.responses++;
      return scrub(value);
    }
    return value;
  };

  // Innertube responses are read through Response.json(), which parses inside
  // the engine and bypasses JSON.parse above.
  if (typeof Response !== "undefined" && Response.prototype) {
    var nativeJson = Response.prototype.json;
    if (typeof nativeJson === "function") {
      Response.prototype.json = function () {
        var self = this;
        return nativeJson.call(self).then(function (value) {
          stats.responses++;
          return scrub(value);
        });
      };
    }
  }

  // Legacy XHR paths read responseText directly.
  if (typeof XMLHttpRequest !== "undefined" && XMLHttpRequest.prototype) {
    var textDescriptor = Object.getOwnPropertyDescriptor(
      XMLHttpRequest.prototype,
      "responseText",
    );
    if (
      textDescriptor &&
      typeof textDescriptor.get === "function" &&
      textDescriptor.configurable
    ) {
      var nativeText = textDescriptor.get;
      Object.defineProperty(XMLHttpRequest.prototype, "responseText", {
        configurable: true,
        enumerable: textDescriptor.enumerable,
        get: function () {
          return scrubText(nativeText.call(this));
        },
      });
    }
  }

  // --- Level 2: network -----------------------------------------------------

  function blockedResponse() {
    if (typeof Response === "function") {
      try {
        return new Response("{}", {
          status: 200,
          headers: { "Content-Type": "application/json" },
        });
      } catch (error) {}
    }
    return {
      ok: true,
      status: 200,
      json: function () {
        return Promise.resolve({});
      },
      text: function () {
        return Promise.resolve("{}");
      },
      clone: blockedResponse,
    };
  }

  if (typeof window.fetch === "function") {
    var nativeFetch = window.fetch;
    window.fetch = function (input) {
      var url =
        typeof input === "string"
          ? input
          : input && typeof input.url === "string"
            ? input.url
            : String(input);
      if (isAdRequest(url)) {
        stats.requests++;
        return Promise.resolve(blockedResponse());
      }
      return nativeFetch.apply(this, arguments);
    };
  }

  if (typeof XMLHttpRequest !== "undefined" && XMLHttpRequest.prototype) {
    var nativeOpen = XMLHttpRequest.prototype.open;
    XMLHttpRequest.prototype.open = function (method, url) {
      if (isAdRequest(url)) {
        stats.requests++;
        // A data: URL removes the network call while keeping the XHR API —
        // callers still get a completed, harmless request.
        arguments[1] = "data:application/json,%7B%7D";
        this.__ytpakeBlocked = true;
      }
      return nativeOpen.apply(this, arguments);
    };
  }

  if (
    typeof navigator !== "undefined" &&
    typeof navigator.sendBeacon === "function"
  ) {
    var nativeBeacon = navigator.sendBeacon;
    navigator.sendBeacon = function (url, data) {
      if (isAdRequest(url)) {
        stats.requests++;
        return true;
      }
      return nativeBeacon.call(this, url, data);
    };
  }

  // --- Level 3: page --------------------------------------------------------

  var sheet = null;
  try {
    sheet = new CSSStyleSheet();
    sheet.replaceSync(filters.css);
  } catch (error) {
    sheet = null;
  }

  function applyCss() {
    if (sheet) {
      try {
        var sheets = document.adoptedStyleSheets || [];
        if (Array.prototype.indexOf.call(sheets, sheet) < 0) {
          document.adoptedStyleSheets = Array.prototype.slice
            .call(sheets)
            .concat([sheet]);
        }
        return;
      } catch (error) {
        // Fall through to a style element.
      }
    }
    if (document.getElementById("ytpake-ad-css")) return;
    var style = document.createElement("style");
    style.id = "ytpake-ad-css";
    style.textContent = filters.css;
    (document.head || document.documentElement || document).appendChild(style);
  }

  // An adopted stylesheet belongs to the document, so it survives YouTube
  // rebuilding its own <head>. Re-apply anyway for engines without
  // constructable stylesheets and for documents that replace theirs.
  function installCss() {
    try {
      applyCss();
      stats.css++;
    } catch (error) {}
  }

  function skipAds() {
    var player = document.querySelector("#movie_player, .html5-video-player");
    if (!player) return;
    var showingAd =
      player.classList.contains("ad-showing") ||
      player.classList.contains("ad-interrupting") ||
      !!document.querySelector(".ytp-ad-player-overlay");
    if (!showingAd) return;

    var skip = document.querySelector(
      ".ytp-ad-skip-button:not([disabled]), .ytp-ad-skip-button-modern, .ytp-ad-skip-button-container button",
    );
    if (skip && typeof skip.click === "function") {
      skip.click();
      stats.skips++;
      return;
    }

    var video = document.querySelector("video");
    if (
      video &&
      isFinite(video.duration) &&
      video.duration > 0 &&
      video.currentTime < video.duration - 0.5
    ) {
      try {
        video.currentTime = video.duration;
        stats.skips++;
      } catch (error) {}
    }
  }

  var lastMutation = 0;
  function onMutation() {
    var now = Date.now();
    if (now - lastMutation < 120) return;
    lastMutation = now;
    installCss();
    skipAds();
  }

  function watch() {
    installCss();
    if (typeof MutationObserver === "function") {
      try {
        var observer = new MutationObserver(onMutation);
        observer.observe(document.documentElement || document, {
          childList: true,
          subtree: true,
          attributes: true,
          attributeFilter: ["class"],
        });
      } catch (error) {}
    }
    document.addEventListener("DOMContentLoaded", installCss, true);
    window.addEventListener("load", installCss, true);
    // A cheap safety net for ads that arrive without a DOM mutation.
    setInterval(skipAds, 500);
  }

  watch();
})();
