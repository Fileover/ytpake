# Extensions

Put **unpacked** Chrome/Edge extensions here, one folder per extension. Each
folder must contain a `manifest.json` at its root:

```
extensions/
  README.md                     this file
  ublock-origin-lite/           manifest.json, ...
  enhancer-for-youtube/         manifest.json, ...
```

The application is YouTube-only. It loads every immediate subfolder that has a
`manifest.json`; a folder without one is ignored. Manifest V3 extensions are
recommended — uBlock Origin Lite is known to work, while classic uBlock Origin
(Manifest V2) can leave requests hanging in WebView2.

Ad blocking does **not** depend on this folder. A built-in blocker is injected
into every YouTube tab at document start and works on a clean install; the
extension is an additional, optional layer. See the main README for details.

## Where this folder goes

| Build     | Location                                                                        |
| --------- | ------------------------------------------------------------------------------- |
| Portable  | `extensions/` next to `YouTube.exe`                                             |
| Installer | `extensions/` inside the installation directory (`%LOCALAPPDATA%\YouTube`)      |
| Override  | `YouTube.exe --extensions-dir C:\path\to\extensions` or `YTPAKE_EXTENSIONS_DIR` |

Every profile shares the same folder. Files added here are copied into the
installer by `tauri.conf.json` (`bundle.resources`), so build after adding an
extension.

## Verify before building

```bash
pnpm extensions:check
```

The checker reports the name, version, manifest version and options page of
each extension, and fails on a malformed manifest. At runtime the same inventory
is written to `%TEMP%\ytpake-trace.log` prefixed with `extensions:`.

To start without any extension: `YouTube.exe --no-extensions`.
To start without the built-in blocker: `YouTube.exe --no-adblock`.

## Licensing

Extensions are separate projects with their own licences (uBlock Origin Lite
and Enhancer for YouTube are GPL-3.0). They are loaded at runtime and are not
part of this repository. Do not commit their contents here.
