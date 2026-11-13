import fs from "node:fs";
import path from "node:path";
import vm from "node:vm";
import { describe, expect, it } from "vitest";

const SOURCE = fs.readFileSync(
  path.join(process.cwd(), "src-tauri", "src", "inject", "adblock.js"),
  "utf8",
);
const FILTERS = JSON.parse(
  fs.readFileSync(path.join(process.cwd(), "filters", "youtube.json"), "utf8"),
);

// The injected script runs at document_start inside WebView2. A small fake
// window is enough to exercise the three blocking layers without a browser.
function loadAdblock(options = {}) {
  let fetchCalls = 0;
  let beaconCalls = 0;
  let observerCallback = null;
  let skipClicks = 0;

  const player = {
    classList: {
      contains: (name) => name === "ad-showing",
    },
  };
  const skipButton = {
    click: () => {
      skipClicks += 1;
    },
  };

  class FakeResponse {
    constructor(body = "{}", init = {}) {
      this.body = body;
      this.status = init.status ?? 200;
    }
    async json() {
      return JSON.parse(this.body);
    }
    async text() {
      return this.body;
    }
    clone() {
      return new FakeResponse(this.body, { status: this.status });
    }
  }

  class FakeXHR {
    constructor() {
      this.readyState = 0;
      this.responseValue = "{}";
    }
    open(method, url) {
      this.method = method;
      this.url = url;
      this.readyState = 1;
    }
    send() {
      this.readyState = 4;
    }
    get responseText() {
      return this.responseValue;
    }
  }

  class FakeCSSStyleSheet {
    constructor() {
      this.rules = "";
    }
    replaceSync(rules) {
      this.rules = rules;
    }
  }

  class FakeMutationObserver {
    constructor(callback) {
      observerCallback = callback;
    }
    observe() {}
  }

  const document = {
    adoptedStyleSheets: [],
    head: { appendChild() {} },
    documentElement: { appendChild() {} },
    getElementById: () => null,
    createElement: () => ({ id: "", textContent: "" }),
    querySelector: (selector) => {
      if (selector.includes("#movie_player")) return player;
      if (selector.includes("ytp-ad-skip-button")) return skipButton;
      return null;
    },
    addEventListener() {},
    appendChild() {},
  };

  const sandbox = {
    location: { hostname: options.hostname ?? "www.youtube.com" },
    document,
    navigator: {
      sendBeacon: () => {
        beaconCalls += 1;
        return true;
      },
    },
    fetch: async () => {
      fetchCalls += 1;
      return new FakeResponse("{}");
    },
    Response: FakeResponse,
    XMLHttpRequest: FakeXHR,
    CSSStyleSheet: FakeCSSStyleSheet,
    MutationObserver: FakeMutationObserver,
    JSON: { parse: JSON.parse, stringify: JSON.stringify },
    Date,
    Promise,
    setInterval: () => 0,
    addEventListener() {},
    console,
  };
  sandbox.window = sandbox;
  // Rust injects this script before adblock.js from filters/youtube.json.
  sandbox.__YTPAKE_ADBLOCK_FILTERS__ = options.filters ?? FILTERS;
  if (options.noFilters) delete sandbox.__YTPAKE_ADBLOCK_FILTERS__;
  if (options.enabled === false) {
    sandbox.__YTPAKE_ADBLOCK_ENABLED__ = false;
  }

  const originals = { fetch: sandbox.fetch, parse: sandbox.JSON.parse };
  vm.createContext(sandbox);
  vm.runInContext(SOURCE, sandbox);

  return {
    sandbox,
    api: sandbox.__ytpakeAdblock,
    document,
    FakeResponse,
    FakeXHR,
    originals,
    fireMutation: () => observerCallback && observerCallback(),
    counts: () => ({ fetchCalls, beaconCalls, skipClicks }),
  };
}

describe("built-in YouTube ad blocker", () => {
  it("classifies ad endpoints without blocking media or player calls", () => {
    const { api } = loadAdblock();
    for (const url of [
      "https://pagead2.googlesyndication.com/pagead/show_ads.js",
      "https://googleads.g.doubleclick.net/pagead/ads",
      "https://www.youtube.com/api/stats/ads?x=1",
      "https://www.youtube.com/youtubei/v1/get_ads",
      "https://example.com/videoplayback?adformat=1",
    ]) {
      expect(api.isAdRequest(url)).toBe(true);
    }
    for (const url of [
      "https://www.youtube.com/youtubei/v1/player?key=abc",
      "https://i.ytimg.com/vi/abc/hqdefault.jpg",
      "https://rr1.googlevideo.com/videoplayback?itag=18",
      "https://www.youtube.com/watch?v=abc",
    ]) {
      expect(api.isAdRequest(url)).toBe(false);
    }
  });

  it("answers ad requests locally and leaves other fetch calls alone", async () => {
    const env = loadAdblock();
    const blocked = await env.sandbox.fetch("https://doubleclick.net/ad");
    expect(blocked.status).toBe(200);
    expect(await blocked.text()).toBe("{}");
    expect(env.api.stats.requests).toBe(1);
    expect(env.counts().fetchCalls).toBe(0);

    await env.sandbox.fetch("https://www.youtube.com/youtubei/v1/player");
    expect(env.counts().fetchCalls).toBe(1);
    expect(env.api.stats.requests).toBe(1);
  });

  it("rewrites ad XHRs and suppresses ad beacons", () => {
    const env = loadAdblock();
    const xhr = new env.FakeXHR();
    xhr.open("POST", "https://adservice.google.com/ads");
    expect(xhr.url).toBe("data:application/json,%7B%7D");
    expect(
      env.sandbox.navigator.sendBeacon("https://googlesyndication.com/x"),
    ).toBe(true);
    expect(env.api.stats.requests).toBe(2);
    expect(env.counts().beaconCalls).toBe(0);

    const normal = new env.FakeXHR();
    normal.open("POST", "https://www.youtube.com/youtubei/v1/player");
    expect(normal.url).toContain("youtube.com");
  });

  it("removes ad placements and ad slots from nested player data", () => {
    const { api } = loadAdblock();
    const data = {
      videoDetails: { title: "A video" },
      adPlacements: [{ adPlacementRenderer: { adVideoId: "ad" } }],
      playerAds: [1],
      nested: {
        deeper: {
          adSlotRenderer: { foo: 1 },
          promotedVideoRenderer: { foo: 2 },
          safe: { keep: true },
        },
      },
    };
    const cleaned = api.scrub(data);
    expect(cleaned.videoDetails.title).toBe("A video");
    expect(cleaned.adPlacements).toBeUndefined();
    expect(cleaned.playerAds).toBeUndefined();
    expect(cleaned.nested.deeper.adSlotRenderer).toBeUndefined();
    expect(cleaned.nested.deeper.promotedVideoRenderer).toBeUndefined();
    expect(cleaned.nested.deeper.safe.keep).toBe(true);
    expect(api.stats.breaks).toBeGreaterThan(0);
  });

  it("scrubs JSON.parse, Response.json and XHR responseText", async () => {
    const env = loadAdblock();
    const payload = JSON.stringify({
      adPlacements: [1],
      content: { adSlotRenderer: {}, keep: 1 },
    });

    expect(env.sandbox.JSON.parse(payload)).toEqual({ content: { keep: 1 } });

    const response = new env.FakeResponse(payload);
    expect(await response.json()).toEqual({ content: { keep: 1 } });

    const xhr = new env.FakeXHR();
    xhr.responseValue = payload;
    expect(JSON.parse(xhr.responseText)).toEqual({ content: { keep: 1 } });
  });

  it("intercepts ytInitialPlayerResponse and ytInitialData assignments", () => {
    const { sandbox } = loadAdblock();
    sandbox.ytInitialPlayerResponse = {
      playabilityStatus: { status: "OK" },
      adPlacements: [{ adVideoId: "ad" }],
      adSlots: [1],
    };
    sandbox.ytInitialData = {
      contents: {
        adSlotRenderer: { foo: 1 },
        videoRenderer: { videoId: "abc" },
      },
    };

    expect(sandbox.ytInitialPlayerResponse.adPlacements).toBeUndefined();
    expect(sandbox.ytInitialPlayerResponse.adSlots).toBeUndefined();
    expect(sandbox.ytInitialPlayerResponse.playabilityStatus.status).toBe("OK");
    expect(sandbox.ytInitialData.contents.adSlotRenderer).toBeUndefined();
    expect(sandbox.ytInitialData.contents.videoRenderer.videoId).toBe("abc");
  });

  it("adopts a stylesheet that covers feed, sidebar and wrapper ads", () => {
    const env = loadAdblock();
    expect(env.document.adoptedStyleSheets).toHaveLength(1);
    const rules = env.document.adoptedStyleSheets[0].rules;
    expect(rules).toContain("ytd-ad-slot-renderer");
    expect(rules).toContain("#masthead-ad");
    expect(rules).toContain("display:none!important");
    // :has() lives in a second rule so an old engine cannot drop the first.
    expect(rules.split("\n")).toHaveLength(2);
    expect(env.api.stats.css).toBe(1);
  });

  it("clicks the skip button when an ad manages to play", () => {
    const env = loadAdblock();
    env.fireMutation();
    expect(env.counts().skipClicks).toBe(1);
    expect(env.api.stats.skips).toBe(1);
  });

  it("does nothing outside YouTube", () => {
    const env = loadAdblock({ hostname: "example.com" });
    expect(env.api).toBeUndefined();
    expect(env.sandbox.fetch).toBe(env.originals.fetch);
    expect(env.sandbox.JSON.parse).toBe(env.originals.parse);
    expect(env.document.adoptedStyleSheets).toHaveLength(0);
  });

  it("uses the runtime catalogue, so a replaced JSON changes the filters", () => {
    const env = loadAdblock({
      filters: {
        version: "test-42",
        adKeys: ["myAdKey"],
        breakKeys: ["myAdKey"],
        urlPatterns: ["my-ad-host"],
        selectors: ["#my-ad"],
        containerSelectors: [],
      },
    });
    expect(env.api.filters.version).toBe("test-42");
    expect(env.api.filters.source).toBe("file");
    expect(env.api.isAdRequest("https://my-ad-host/track")).toBe(true);
    // Replacement, not just an addition: the shipped rules are gone.
    expect(env.api.isAdRequest("https://doubleclick.net/ad")).toBe(false);
    expect(env.api.css).toContain("#my-ad");
    expect(env.api.css).not.toContain("ytd-ad-slot-renderer");
    expect(env.api.scrub({ myAdKey: [1], keep: 1 })).toEqual({ keep: 1 });
    expect(env.api.stats.breaks).toBeGreaterThan(0);
  });

  it("keeps an emergency fallback if the filter script never ran", () => {
    const env = loadAdblock({ noFilters: true });
    expect(env.api.filters.source).toBe("fallback");
    expect(env.api.isAdRequest("https://doubleclick.net/ad")).toBe(true);
    expect(env.document.adoptedStyleSheets).toHaveLength(1);
  });

  it("--no-adblock installs no hooks at all", async () => {
    const env = loadAdblock({ enabled: false });
    expect(env.api.enabled).toBe(false);
    expect(env.sandbox.fetch).toBe(env.originals.fetch);
    expect(env.sandbox.JSON.parse).toBe(env.originals.parse);
    expect(env.document.adoptedStyleSheets).toHaveLength(0);
    await env.sandbox.fetch("https://doubleclick.net/ad");
    expect(env.counts().fetchCalls).toBe(1);
    expect(env.api.stats.requests).toBe(0);
  });
});
