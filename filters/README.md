# Ad filter updates

`youtube.json` is the filter catalogue for the built-in ad blocker. It is data,
not compiled code: the application reads it on every launch and injects it into
each tab before YouTube's own scripts run.

## Updating without a rebuild

1. Edit `filters/youtube.json` (or download a newer copy).
2. Validate it: `pnpm filters:check` (or `node scripts/check-filters.mjs --file new.json`).
3. Put the file at `filters/youtube.json` relative to the application data
   folder (`%APPDATA%\com.pake.youtube.tabs\filters\` for an installed build)
   or next to `YouTube.exe` for a portable build.
4. Restart the application. Check `%TEMP%\ytpake-trace.log` for a line like
   `adblock filters: app data ... (version 2026.11.13-1)`.

Precedence, first valid file wins:

| Order | Source                                     | Purpose                          |
| ----- | ------------------------------------------ | -------------------------------- |
| 1     | `--filters <path>` / `YTPAKE_FILTERS_FILE` | Test a candidate file            |
| 2     | `<app data>/filters/youtube.json`          | User update, survives reinstalls |
| 3     | `<exe dir>/filters/youtube.json`           | Copy shipped in the installer    |
| 4     | JSON compiled into the binary              | Baseline, always available       |

A malformed file is skipped with a trace line; the blocker keeps running with
the next source. `adblock.js` also carries a small emergency fallback in case no
catalogue is injected at all.

## Format

| Field                | What it is                                                       |
| -------------------- | ---------------------------------------------------------------- |
| `version`            | Label written to the trace log; format `YYYY.MM.DD-N`            |
| `updated`            | ISO date, documentation only                                     |
| `adKeys`             | Object keys removed recursively from player/Innertube data       |
| `breakKeys`          | Subset of `adKeys` counted as blocked video ad breaks            |
| `urlPatterns`        | JavaScript regex sources for ad-only requests (case-insensitive) |
| `selectors`          | CSS selectors for ad slots rendered into the page                |
| `containerSelectors` | Selectors using `:has()`, kept in a separate CSS rule            |

Rules to follow when editing:

- Never add a pattern that matches `youtubei/v1/player`, `ytimg.com` or
  `googlevideo.com` — that breaks playback. `pnpm filters:check` rejects it.
- Selectors are joined into one rule; `:has()` selectors belong in
  `containerSelectors` so an older engine does not invalidate the whole rule.
- Adding a key to `adKeys` also extends the fast text pre-check used before
  scrubbing JSON, so no code change is needed.

## Automatic updates

The application does not fetch filter lists from the network. The update path is
deliberate: a JSON file the user controls, with a compiled-in baseline. If you
want lists that refresh themselves, install uBlock Origin Lite (MV3) as an
unpacked extension — it updates its own rulesets and acts as a second layer on
top of the built-in blocker.
