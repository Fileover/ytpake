// Injected into every content webview before the page's own scripts run.
//
// uBlock Origin Lite is loaded and does block requests here — a script from an
// ad host fails to load — but its cosmetic filtering does nothing in WebView2:
// an element EasyList is supposed to hide keeps its box, before and after a
// reload. The ads that survive are the ones YouTube renders into the page
// itself, so there is no request to cancel; the only way to remove them is to
// hide them.
//
// The stylesheet is adopted by the document rather than appended as a <style>
// element: the page rebuilds its own head while it boots, and a style element
// added this early does not survive that. Adopted sheets belong to the
// document, so they do.
//
// The player's own ad UI is deliberately left alone. Blocking already covers
// the video ads, and hiding the module when one does slip through would leave a
// black player with no countdown and no skip button.
(function () {
  if (!/(^|\.)youtube\.com$/.test(location.hostname)) return;
  window.__ytpakeAds = "start";

  var css = [
    // Feed, search results, watch page sidebar.
    "ytd-ad-slot-renderer",
    "ytd-display-ad-renderer",
    "ytd-promoted-sparkles-web-renderer",
    "ytd-promoted-video-renderer",
    "ytd-in-feed-ad-layout-renderer",
    "ytd-search-pyv-renderer",
    // Banner across the top of the home page, and the column beside the video.
    "#masthead-ad",
    "#player-ads",
    // Promos that YouTube inserts between sections.
    "ytd-banner-promo-renderer",
    "ytd-companion-slot-renderer",
    "ytd-action-companion-ad-renderer",
  ].join(",") + "{display:none !important}";

  var sheet = null;
  try {
    sheet = new CSSStyleSheet();
    sheet.replaceSync(css);
  } catch (error) {
    sheet = null;
  }

  function hide() {
    if (sheet) {
      var sheets = document.adoptedStyleSheets;
      if (sheets.indexOf(sheet) < 0) {
        document.adoptedStyleSheets = sheets.concat(sheet);
      }
      window.__ytpakeAds = "adopted:" + document.adoptedStyleSheets.length;
      return;
    }
    // Fallback for an engine without constructable stylesheets.
    if (document.getElementById("ytpake-ad-css")) return;
    var style = document.createElement("style");
    style.id = "ytpake-ad-css";
    style.textContent = css;
    (document.head || document.documentElement).appendChild(style);
    window.__ytpakeAds = "element";
  }

  try {
    hide();
  } catch (error) {
    window.__ytpakeAds = "error:" + error.message;
  }
  document.addEventListener("DOMContentLoaded", hide, true);
  window.addEventListener("load", hide, true);
})();
