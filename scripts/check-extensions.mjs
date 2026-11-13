#!/usr/bin/env node
// Validate the unpacked extensions that will be copied into the installer.
// Run with `pnpm extensions:check`, or pass --dir / set YTPAKE_EXTENSIONS_DIR.

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

function option(name) {
  const index = process.argv.indexOf(name);
  if (index >= 0) return process.argv[index + 1];
  const inline = process.argv.find((arg) => arg.startsWith(`${name}=`));
  return inline ? inline.slice(name.length + 1) : undefined;
}

const asJson = process.argv.includes("--json");

function resolveDir() {
  const candidate = option("--dir") ?? process.env.YTPAKE_EXTENSIONS_DIR;
  const base = candidate ?? path.join(ROOT, "extensions");
  return path.isAbsolute(base) ? base : path.resolve(process.cwd(), base);
}

function readManifest(folder) {
  const file = path.join(folder, "manifest.json");
  const raw = fs.readFileSync(file, "utf8");
  return JSON.parse(raw);
}

function inspect(folder) {
  const entry = { folder: path.basename(folder), problems: [], warnings: [] };
  try {
    entry.manifest = readManifest(folder);
  } catch (error) {
    entry.problems.push(
      `manifest.json is unreadable or invalid: ${error.message}`,
    );
    return entry;
  }

  const manifest = entry.manifest;
  entry.name = manifest.name ?? "(unnamed)";
  entry.version = manifest.version ?? "unknown";
  entry.manifestVersion = manifest.manifest_version ?? 2;
  entry.optionsPage = Boolean(
    manifest.options_ui?.page ?? manifest.options_page,
  );

  if (typeof manifest.name !== "string") entry.problems.push('missing "name"');
  if (typeof manifest.version !== "string")
    entry.problems.push('missing "version"');
  if (![2, 3].includes(entry.manifestVersion)) {
    entry.problems.push(
      `unsupported manifest_version ${entry.manifestVersion}`,
    );
  }
  if (entry.manifestVersion === 2) {
    entry.warnings.push(
      "Manifest V2 extensions can hang requests in WebView2; prefer MV3",
    );
  }
  if (!entry.optionsPage) {
    entry.warnings.push(
      "no options page, so it will not appear in the extensions menu",
    );
  }
  if (manifest.background?.service_worker && entry.manifestVersion === 2) {
    entry.warnings.push("a service worker in an MV2 manifest will not run");
  }
  return entry;
}

const dir = resolveDir();
const results = [];
let missing = false;

if (!fs.existsSync(dir)) {
  missing = true;
  results.push({
    folder: path.basename(dir),
    problems: [`folder not found: ${dir}`],
    warnings: [],
  });
} else {
  const entries = fs
    .readdirSync(dir, { withFileTypes: true })
    .filter((item) => item.isDirectory())
    .sort((a, b) => a.name.localeCompare(b.name));

  for (const item of entries) {
    const folder = path.join(dir, item.name);
    if (!fs.existsSync(path.join(folder, "manifest.json"))) {
      results.push({
        folder: item.name,
        problems: [],
        warnings: ["no manifest.json at the folder root (ignored at runtime)"],
      });
      continue;
    }
    results.push(inspect(folder));
  }
}

const extensions = results.filter((item) => item.manifest);
const problems = results.flatMap((item) =>
  item.problems.map((problem) => `${item.folder}: ${problem}`),
);

if (asJson) {
  console.log(JSON.stringify({ folder: dir, extensions, problems }, null, 2));
} else {
  console.log(`Extensions folder: ${dir}`);
  if (extensions.length === 0) {
    console.log("No extensions found. The built-in ad blocker still works.");
  }
  for (const item of results) {
    if (item.manifest) {
      console.log(
        `  ${item.name} v${item.version} (MV${item.manifestVersion}) ` +
          `[${item.folder}] options=${item.optionsPage}`,
      );
    }
    for (const warning of item.warnings) console.log(`    warning: ${warning}`);
    for (const problem of item.problems) console.log(`    error:   ${problem}`);
  }
  console.log(
    `${extensions.length} extension(s), ${problems.length} error(s).`,
  );
}

if (missing || problems.length > 0) process.exitCode = 1;
