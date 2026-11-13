import fs from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";

const ROOT = process.cwd();
const read = (file) => fs.readFileSync(path.join(ROOT, file), "utf8");
const FILTERS = JSON.parse(read("filters/youtube.json"));

describe("ad filter catalogue", () => {
  it("has the shape adblock.js expects", () => {
    for (const field of [
      "adKeys",
      "breakKeys",
      "urlPatterns",
      "selectors",
      "containerSelectors",
    ]) {
      expect(Array.isArray(FILTERS[field])).toBe(true);
      expect(FILTERS[field].every((item) => typeof item === "string")).toBe(
        true,
      );
      expect(FILTERS[field].length).toBeGreaterThan(0);
    }
    expect(typeof FILTERS.version).toBe("string");
    expect(FILTERS.version).toMatch(/^\d{4}\.\d{2}\.\d{2}-\d+$/);
  });

  it("every URL pattern compiles and every selector is a non-empty string", () => {
    for (const pattern of FILTERS.urlPatterns) {
      expect(() => new RegExp(pattern, "i")).not.toThrow();
      expect(pattern.length).toBeGreaterThan(0);
    }
    for (const selector of [
      ...FILTERS.selectors,
      ...FILTERS.containerSelectors,
    ]) {
      expect(selector.trim()).toBe(selector);
      expect(selector.length).toBeGreaterThan(0);
    }
  });

  it("does not block YouTube media, thumbnails or player calls", () => {
    const rules = FILTERS.urlPatterns.map(
      (pattern) => new RegExp(pattern, "i"),
    );
    for (const url of [
      "https://www.youtube.com/youtubei/v1/player?key=abc",
      "https://i.ytimg.com/vi/abc/hqdefault.jpg",
      "https://rr1.googlevideo.com/videoplayback?itag=18",
      "https://www.youtube.com/watch?v=abc",
    ]) {
      expect(
        rules.some((rule) => rule.test(url)),
        url,
      ).toBe(false);
    }
    for (const url of [
      "https://pagead2.googlesyndication.com/pagead/show_ads.js",
      "https://googleads.g.doubleclick.net/pagead/ads",
      "https://www.youtube.com/api/stats/ads",
    ]) {
      expect(
        rules.some((rule) => rule.test(url)),
        url,
      ).toBe(true);
    }
  });

  it("has no duplicate keys or selectors", () => {
    for (const field of [
      "adKeys",
      "breakKeys",
      "urlPatterns",
      "selectors",
      "containerSelectors",
    ]) {
      expect(new Set(FILTERS[field]).size, `duplicate in ${field}`).toBe(
        FILTERS[field].length,
      );
    }
    for (const key of FILTERS.breakKeys) {
      expect(FILTERS.adKeys).toContain(key);
    }
  });

  it("is shipped as a bundle resource and is not gitignored", () => {
    const tauriConfig = JSON.parse(read("src-tauri/tauri.conf.json"));
    expect(tauriConfig.bundle.resources["../filters/"]).toBe("filters/");
    expect(read(".gitignore")).toContain("extensions/*");
    expect(read(".gitignore")).not.toContain("filters/*");
  });
});
