//! Extension discovery and settings pages.
//!
//! Extensions are unpacked folders dropped beside the executable. WebView2
//! loads every immediate subfolder that holds a `manifest.json`, and assigns
//! each one an id derived from its path; that id is only recorded in the
//! browser profile, so it is read back from there.
//!
//! A settings page cannot simply be navigated to: WebView2 refuses
//! `chrome-extension://` as a top-level document. It can, however, be embedded
//! in an iframe, which is what the tab strip does — provided the extension
//! declares the page as web accessible. Extensions that do not are granted that
//! access here, which is what makes the feature work for any extension instead
//! of a hand-picked one.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use serde::Serialize;
use serde_json::{json, Map, Value};

/// One extension that can show a settings page.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionInfo {
    pub name: String,
    pub url: String,
}

/// One unpacked extension found in the extensions folder.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadedExtension {
    pub name: String,
    pub version: String,
    pub manifest_version: u8,
    pub folder: String,
    pub options_page: bool,
}

/// Every folder holding a readable manifest, ordered by name.
///
/// This is intentionally independent of the WebView2 profile: it reports what
/// the app *will* try to load, including extensions without a settings page,
/// which the tab strip menu never lists.
pub fn discover(folder: &Path) -> Vec<LoadedExtension> {
    let mut found = Vec::new();
    let Ok(entries) = fs::read_dir(folder) else {
        return found;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Ok(text) = fs::read_to_string(path.join("manifest.json")) else {
            continue;
        };
        let Ok(manifest) = serde_json::from_str::<Value>(&text) else {
            continue;
        };

        found.push(LoadedExtension {
            name: display_name(&path, &manifest),
            version: manifest
                .get("version")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string(),
            manifest_version: manifest
                .get("manifest_version")
                .and_then(Value::as_u64)
                .unwrap_or(2) as u8,
            folder: path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            options_page: settings_page(&manifest).is_some(),
        });
    }

    found.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    found
}

/// Write the extension inventory to the startup trace so a missing or broken
/// extension folder is visible without attaching a debugger.
pub fn log_discovered(folder: &Path) {
    use crate::app::tabs::trace;

    let found = discover(folder);
    if found.is_empty() {
        trace(&format!(
            "extensions: none in {} (the built-in ad blocker is still active)",
            folder.display()
        ));
        return;
    }

    for extension in &found {
        trace(&format!(
            "extensions: {} v{} MV{} [{}] options={}",
            extension.name,
            extension.version,
            extension.manifest_version,
            extension.folder,
            extension.options_page
        ));
    }
}

/// Origins that are allowed to embed a settings page.
const SETTINGS_ORIGINS: [&str; 2] = ["https://www.youtube.com/*", "https://*.youtube.com/*"];

/// Grant embeddable access to every settings page in `folder`.
///
/// Must run before WebView2 loads the extensions: a manifest patched afterwards
/// has no effect on the copy the browser is already running, and an iframe
/// pointing at a page that is not web accessible takes the whole tab down.
pub fn prepare(folder: &Path) {
    let Ok(entries) = fs::read_dir(folder) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Ok(text) = fs::read_to_string(path.join("manifest.json")) else {
            continue;
        };
        let Ok(manifest) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        if let Some(page) = settings_page(&manifest) {
            let _ = grant_page_access(&path, &manifest, &page);
        }
    }
}

/// Every extension in `folder` that declares a settings page, ordered by name.
pub fn settings_pages(webview_dir: &Path, folder: &Path) -> Vec<ExtensionInfo> {
    let ids = assigned_ids(webview_dir);
    let mut found = Vec::new();

    let Ok(entries) = fs::read_dir(folder) else {
        return found;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Ok(text) = fs::read_to_string(path.join("manifest.json")) else {
            continue;
        };
        let Ok(manifest) = serde_json::from_str::<Value>(&text) else {
            continue;
        };

        let Some(page) = settings_page(&manifest) else {
            continue;
        };
        let Some(id) = ids.get(&normalize(&path)) else {
            // The extension has not been loaded into this profile yet, so its
            // id is unknown. It appears here after the first run.
            continue;
        };
        if !grant_page_access(&path, &manifest, &page) {
            continue;
        }

        found.push(ExtensionInfo {
            name: display_name(&path, &manifest),
            url: format!("chrome-extension://{id}/{page}"),
        });
    }

    found.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    found.dedup_by(|a, b| a.url == b.url);
    found
}

/// `options_ui.page`, then the older `options_page`, relative to the extension.
fn settings_page(manifest: &Value) -> Option<String> {
    let page = manifest
        .pointer("/options_ui/page")
        .and_then(Value::as_str)
        .or_else(|| manifest.get("options_page").and_then(Value::as_str))?;

    let page = page.trim_start_matches('/').trim();
    (!page.is_empty()).then(|| page.to_string())
}

/// Human readable name of an extension.
///
/// Most extensions ship `"name": "__MSG_extName__"` and leave the real name to
/// `_locales/<locale>/messages.json`, which only the browser resolves. Reading
/// the catalogue here keeps the menu showing "uBlock Origin Lite" instead of
/// the raw key, and works for any extension that localises its name.
fn display_name(folder: &Path, manifest: &Value) -> String {
    for key in ["name", "short_name"] {
        if let Some(raw) = manifest.get(key).and_then(Value::as_str) {
            if let Some(name) = resolve_message(folder, manifest, raw) {
                return name;
            }
        }
    }

    // Nothing usable in the manifest: the folder name is still better than a
    // raw `__MSG_...__` token.
    folder
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Extensão".to_string())
}

/// Expand a manifest string, which may be a `__MSG_key__` placeholder.
fn resolve_message(folder: &Path, manifest: &Value, raw: &str) -> Option<String> {
    let Some(key) = raw
        .strip_prefix("__MSG_")
        .and_then(|rest| rest.strip_suffix("__"))
    else {
        let raw = raw.trim();
        return (!raw.is_empty()).then(|| raw.to_string());
    };

    let mut locales: Vec<String> = Vec::new();
    if let Some(default) = manifest.get("default_locale").and_then(Value::as_str) {
        locales.push(default.to_string());
    }
    locales.push("en".to_string());
    if let Some(any) = first_locale(folder) {
        locales.push(any);
    }

    for locale in locales {
        let Ok(text) =
            fs::read_to_string(folder.join("_locales").join(&locale).join("messages.json"))
        else {
            continue;
        };
        let Ok(messages) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        if let Some(message) = messages
            .get(key)
            .and_then(|entry| entry.get("message"))
            .and_then(Value::as_str)
        {
            let message = message.trim();
            if !message.is_empty() {
                return Some(message.to_string());
            }
        }
    }

    None
}

/// Any locale folder, used as a last resort when the declared ones are absent.
fn first_locale(folder: &Path) -> Option<String> {
    fs::read_dir(folder.join("_locales"))
        .ok()?
        .flatten()
        .find(|entry| entry.path().is_dir())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
}

/// Make the settings page embeddable by the configured site.
///
/// Returns false when the manifest cannot be updated, which is the case for a
/// read-only folder; the caller then leaves the extension out of the menu
/// instead of opening a page that would never render.
fn grant_page_access(folder: &Path, manifest: &Value, page: &str) -> bool {
    if already_accessible(manifest, page) {
        return true;
    }

    let mut updated = manifest.clone();
    let manifest_version = updated
        .get("manifest_version")
        .and_then(Value::as_u64)
        .unwrap_or(2);

    let entry = if manifest_version >= 3 {
        json!({
            "resources": [page],
            "matches": SETTINGS_ORIGINS,
        })
    } else {
        // Manifest V2 lists plain paths; the browser applies them to every
        // origin, so the page simply has to be named.
        Value::String(page.to_string())
    };

    let slot = updated.as_object_mut().map(|object| {
        object
            .entry("web_accessible_resources")
            .or_insert_with(|| Value::Array(Vec::new()))
    });
    let Some(slot) = slot else {
        return false;
    };
    match slot {
        Value::Array(items) => items.push(entry),
        _ => *slot = Value::Array(vec![entry]),
    }

    let Ok(text) = serde_json::to_string_pretty(&updated) else {
        return false;
    };
    fs::write(folder.join("manifest.json"), text).is_ok()
}

fn already_accessible(manifest: &Value, page: &str) -> bool {
    let Some(items) = manifest
        .get("web_accessible_resources")
        .and_then(Value::as_array)
    else {
        return false;
    };

    items.iter().any(|item| match item {
        Value::String(path) => covers(path, page),
        Value::Object(entry) => {
            accessible_from_youtube(entry)
                && entry
                    .get("resources")
                    .and_then(Value::as_array)
                    .map(|resources| {
                        resources
                            .iter()
                            .filter_map(Value::as_str)
                            .any(|resource| covers(resource, page))
                    })
                    .unwrap_or(false)
        }
        _ => false,
    })
}

/// True when an MV3 resource entry also allows the YouTube tab to embed it.
///
/// MV3 scopes `web_accessible_resources` to `matches`. A settings page listed
/// only for an unrelated origin is still unreachable from our tab strip, so it
/// needs a separate entry carrying `SETTINGS_ORIGINS`. MV2 string entries have
/// no such restriction and are handled by `covers` alone.
fn accessible_from_youtube(entry: &Map<String, Value>) -> bool {
    let Some(matches) = entry.get("matches").and_then(Value::as_array) else {
        // Not valid MV3, but do not reject a page the browser might accept.
        return true;
    };

    matches.iter().filter_map(Value::as_str).any(|origin| {
        if origin == "<all_urls>" || origin == "*://*/*" {
            return true;
        }
        let Some(rest) = origin.split_once("://").map(|(_, rest)| rest) else {
            return false;
        };
        let host = rest.split('/').next().unwrap_or_default();
        // `*.youtube.com` also ends with `.youtube.com`, so wildcards are
        // covered without accepting lookalikes such as `notyoutube.com.evil`.
        host == "youtube.com" || host.ends_with(".youtube.com")
    })
}

/// Match a `web_accessible_resources` path against a settings page path.
///
/// Chrome resource paths are globs: `*` matches any run of characters, so
/// `options/*`, `options/*.html` and `*` all cover `options/index.html`.
fn covers(resource: &str, page: &str) -> bool {
    glob_matches(
        resource.trim_start_matches('/'),
        page.trim_start_matches('/'),
    )
}

/// Minimal `*` glob matcher; the manifest pattern syntax has no `?`.
fn glob_matches(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let text: Vec<char> = text.chars().collect();
    let (mut pattern_index, mut text_index) = (0, 0);
    let mut last_star: Option<usize> = None;
    let mut backtrack = 0;

    while text_index < text.len() {
        if pattern_index < pattern.len() && pattern[pattern_index] == text[text_index] {
            pattern_index += 1;
            text_index += 1;
        } else if pattern_index < pattern.len() && pattern[pattern_index] == '*' {
            last_star = Some(pattern_index);
            backtrack = text_index;
            pattern_index += 1;
        } else if let Some(star) = last_star {
            backtrack += 1;
            text_index = backtrack;
            pattern_index = star + 1;
        } else {
            return false;
        }
    }

    while pattern_index < pattern.len() && pattern[pattern_index] == '*' {
        pattern_index += 1;
    }
    pattern_index == pattern.len()
}

/// Map of normalised extension folder -> id, read from the WebView2 profile.
fn assigned_ids(webview_dir: &Path) -> HashMap<String, String> {
    let mut ids = HashMap::new();

    for file in ["Secure Preferences", "Preferences"] {
        let path = webview_dir.join("EBWebView").join("Default").join(file);
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(json) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        let Some(settings) = json
            .pointer("/extensions/settings")
            .and_then(Value::as_object)
        else {
            continue;
        };
        for (id, value) in settings {
            if let Some(folder) = value.get("path").and_then(Value::as_str) {
                ids.insert(normalize(Path::new(folder)), id.clone());
            }
        }
    }

    ids
}

fn normalize(path: &Path) -> String {
    path.to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase()
}

/// True when the folder holds at least one unpacked extension.
pub fn has_extensions(folder: &Path) -> bool {
    fs::read_dir(folder)
        .map(|entries| {
            entries.flatten().any(|entry| {
                let path = entry.path();
                path.is_dir() && path.join("manifest.json").is_file()
            })
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_page_prefers_options_ui_and_trims_the_path() {
        let manifest = json!({
            "options_ui": { "page": "/settings/index.html" },
            "options_page": "legacy.html"
        });
        assert_eq!(
            settings_page(&manifest),
            Some("settings/index.html".to_string())
        );

        let legacy = json!({ "options_page": "/legacy.html" });
        assert_eq!(settings_page(&legacy), Some("legacy.html".to_string()));
        assert_eq!(settings_page(&json!({})), None);
    }

    #[test]
    fn web_accessible_resources_cover_exact_paths_and_prefix_wildcards() {
        let manifest = json!({
            "web_accessible_resources": [
                { "resources": ["options/*.html"], "matches": ["https://www.youtube.com/*"] },
                "settings/legacy.html"
            ]
        });
        assert!(already_accessible(&manifest, "options/index.html"));
        assert!(already_accessible(&manifest, "settings/legacy.html"));
        assert!(!already_accessible(&manifest, "dashboard.html"));
    }

    #[test]
    fn web_accessible_resource_globs_cover_directory_and_filename_wildcards() {
        let all = json!({ "web_accessible_resources": ["*"] });
        assert!(already_accessible(&all, "options/index.html"));

        let directory = json!({
            "web_accessible_resources": [
                { "resources": ["options/*"], "matches": ["https://www.youtube.com/*"] }
            ]
        });
        assert!(already_accessible(&directory, "options/index.html"));
        assert!(!already_accessible(&directory, "assets/index.html"));

        let extension = json!({ "web_accessible_resources": ["assets/*.js"] });
        assert!(!already_accessible(&extension, "options/index.html"));
        assert!(already_accessible(&extension, "assets/main.js"));
    }

    #[test]
    fn an_entry_scoped_to_another_origin_needs_youtube_access_granted() {
        let manifest = json!({
            "manifest_version": 3,
            "web_accessible_resources": [
                { "resources": ["options.html"], "matches": ["https://example.com/*"] }
            ]
        });
        assert!(!already_accessible(&manifest, "options.html"));

        let youtube = json!({
            "manifest_version": 3,
            "web_accessible_resources": [
                { "resources": ["options.html"], "matches": ["https://*.youtube.com/*"] }
            ]
        });
        assert!(already_accessible(&youtube, "options.html"));

        let lookalike = json!({
            "manifest_version": 3,
            "web_accessible_resources": [
                { "resources": ["options.html"], "matches": ["https://notyoutube.com.evil/*"] }
            ]
        });
        assert!(!already_accessible(&lookalike, "options.html"));

        let global = json!({
            "manifest_version": 3,
            "web_accessible_resources": [
                { "resources": ["options.html"], "matches": ["<all_urls>"] }
            ]
        });
        assert!(already_accessible(&global, "options.html"));
    }

    #[test]
    fn an_mv3_settings_page_is_granted_youtube_only_access() {
        use std::time::{SystemTime, UNIX_EPOCH};
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "ytpake-extension-test-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("temp folder");

        let manifest = json!({
            "manifest_version": 3,
            "name": "Test extension",
            "version": "1.0",
            "options_ui": { "page": "options.html" }
        });
        assert!(grant_page_access(&dir, &manifest, "options.html"));

        let updated: Value =
            serde_json::from_str(&fs::read_to_string(dir.join("manifest.json")).expect("manifest"))
                .expect("valid JSON");
        let entry = &updated["web_accessible_resources"][0];
        assert_eq!(entry["resources"][0], json!("options.html"));
        assert_eq!(
            entry["matches"],
            serde_json::to_value(SETTINGS_ORIGINS).expect("origins")
        );
        assert!(already_accessible(&updated, "options.html"));

        let _ = fs::remove_dir_all(&dir);
    }
}
