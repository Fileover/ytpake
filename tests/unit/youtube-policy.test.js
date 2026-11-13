import fs from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";

const read = (file) => fs.readFileSync(path.join(process.cwd(), file), "utf8");

function quotedStrings(source) {
  return [...source.matchAll(/"([^"]+)"/g)].map((match) => match[1]);
}

describe("YouTube-only policy", () => {
  it("keeps the Rust allowlist and the page-side allowlist in sync", () => {
    const rust = read("src-tauri/src/app/youtube.rs");
    const bridge = read("src-tauri/src/inject/tab_bridge.js");
    const rustBlock = rust.match(
      /const NAVIGATION_DOMAINS[^=]*=\s*&\[([\s\S]*?)\];/,
    );
    const bridgeBlock = bridge.match(
      /var ALLOWED_DOMAINS\s*=\s*\[([\s\S]*?)\];/,
    );
    expect(rustBlock).toBeTruthy();
    expect(bridgeBlock).toBeTruthy();

    const rustDomains = quotedStrings(rustBlock[1]).sort();
    const bridgeDomains = quotedStrings(bridgeBlock[1]).sort();
    expect(rustDomains.length).toBeGreaterThan(0);
    expect(bridgeDomains).toEqual(rustDomains);
  });

  it("registers the external-link command in the ACL manifest and capability", () => {
    const manifest = read("src-tauri/build.rs");
    const capability = JSON.parse(read("src-tauri/capabilities/default.json"));
    expect(manifest).toContain('"tabs_open_external"');
    expect(capability.permissions).toContain("allow-tabs-open-external");
  });

  it("ships both runtime catalogues as bundle resources", () => {
    const config = JSON.parse(read("src-tauri/tauri.conf.json"));
    expect(config.bundle.resources).toEqual({
      "../extensions/": "extensions/",
      "../filters/": "filters/",
    });
  });
});
