#!/usr/bin/env node
// Validate a filter catalogue before deploying it next to the executable.
//   pnpm filters:check                       checks filters/youtube.json
//   pnpm filters:check -- --file new.json    checks a candidate update

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

const file = option("--file") ?? process.env.YTPAKE_FILTERS_FILE;
const target = path.isAbsolute(file ?? "")
  ? (file ?? "")
  : path.resolve(
      process.cwd(),
      file ?? path.join(ROOT, "filters", "youtube.json"),
    );

const REQUIRED = [
  "adKeys",
  "breakKeys",
  "urlPatterns",
  "selectors",
  "containerSelectors",
];
const problems = [];
let catalogue;

try {
  catalogue = JSON.parse(fs.readFileSync(target, "utf8"));
} catch (error) {
  console.error(`Cannot read ${target}: ${error.message}`);
  process.exit(1);
}

for (const field of REQUIRED) {
  const list = catalogue[field];
  if (!Array.isArray(list)) {
    problems.push(`${field} must be an array`);
    continue;
  }
  if (!list.length) problems.push(`${field} is empty`);
  if (!list.every((item) => typeof item === "string" && item.trim())) {
    problems.push(`${field} contains a non-string or empty entry`);
  }
  if (new Set(list).size !== list.length)
    problems.push(`${field} has duplicates`);
}
if (
  typeof catalogue.version !== "string" ||
  !/^\d{4}\.\d{2}\.\d{2}-\d+$/.test(catalogue.version)
) {
  problems.push("version must look like YYYY.MM.DD-N");
}

const rules = (catalogue.urlPatterns ?? []).flatMap((pattern, index) => {
  try {
    return [new RegExp(pattern, "i")];
  } catch (error) {
    problems.push(
      `urlPatterns[${index}] is not a valid regex: ${error.message}`,
    );
    return [];
  }
});

for (const key of catalogue.breakKeys ?? []) {
  if (!(catalogue.adKeys ?? []).includes(key)) {
    problems.push(`breakKeys entry ${key} is missing from adKeys`);
  }
}

// A new rule must never swallow the requests YouTube needs to play a video.
const MUST_PASS = [
  "https://www.youtube.com/youtubei/v1/player?key=abc",
  "https://i.ytimg.com/vi/abc/hqdefault.jpg",
  "https://rr1.googlevideo.com/videoplayback?itag=18",
  "https://www.youtube.com/watch?v=abc",
];
const MUST_BLOCK = [
  "https://pagead2.googlesyndication.com/pagead/show_ads.js",
  "https://googleads.g.doubleclick.net/pagead/ads",
  "https://www.youtube.com/api/stats/ads",
];
for (const url of MUST_PASS) {
  if (rules.some((rule) => rule.test(url)))
    problems.push(`rule blocks a required URL: ${url}`);
}
for (const url of MUST_BLOCK) {
  if (!rules.some((rule) => rule.test(url)))
    problems.push(`no rule blocks: ${url}`);
}

console.log(`File:    ${target}`);
console.log(`Version: ${catalogue.version ?? "(missing)"}`);
if (catalogue.updated) console.log(`Updated: ${catalogue.updated}`);
for (const field of REQUIRED) {
  console.log(`  ${field.padEnd(20)} ${(catalogue[field] ?? []).length}`);
}
if (problems.length) {
  for (const problem of problems) console.error(`  error: ${problem}`);
  console.error(`${problems.length} problem(s) found.`);
  process.exitCode = 1;
} else {
  console.log(
    "OK. Copy this file to filters/youtube.json beside the executable and restart.",
  );
}
