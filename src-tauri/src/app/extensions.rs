//! Unpacked browser extension discovery, manifest preparation, and WebView2 profile synchronization.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use serde::Serialize;
use serde_json::{json, Map, Value};

use crate::app::tabs::trace;

/// One extension that can show a settings page in the tab strip menu.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionInfo {
    pub name: String,
    pub url: String,
}

/// Cached metadata for one unpacked extension discovered in `.\extensions`.
#[derive(Clone, Debug)]
pub struct DiscoveredExtension {
    pub name: String,
    pub version: String,
    pub manifest_version: u8,
    pub folder: String,
    pub path: PathBuf,
    pub normalized_path: String,
    pub settings_page: Option<String>,
}

/// Synchronization plan computed before WebView2 starts.
#[derive(Clone, Debug, Default)]
pub struct ProfileExtensionPlan {
    pub discovered: Vec<DiscoveredExtension>,
    pub missing_folders: Vec<PathBuf>,
    pub stale_ids: Vec<String>,
}

impl ProfileExtensionPlan {
    pub fn has_extensions(&self) -> bool {
        !self.discovered.is_empty()
    }
}

#[derive(Default)]
struct RuntimeRegistry {
    discovered: Vec<DiscoveredExtension>,
    /// Map of `normalized_path -> extension_id`.
    ids_by_path: HashMap<String, String>,
}

static REGISTRY: OnceLock<Mutex<RuntimeRegistry>> = OnceLock::new();

fn registry() -> &'static Mutex<RuntimeRegistry> {
    REGISTRY.get_or_init(|| Mutex::new(RuntimeRegistry::default()))
}

/// Origins allowed to embed an extension settings page in an iframe.
const SETTINGS_ORIGINS: [&str; 2] = ["https://www.youtube.com/*", "https://*.youtube.com/*"];

/// Single-pass scan of `folder`: prepares settings page access in `manifest.json`
/// (writing only when missing), logs discovered extensions, and inspects the
/// WebView2 profile preferences to find existing extension IDs.
pub fn prepare_and_inspect(folder: &Path, webview_dir: &Path) -> ProfileExtensionPlan {
    let mut discovered = Vec::new();

    if let Ok(entries) = fs::read_dir(folder) {
        for entry in entries.flatten() {
            if !entry.file_type().is_ok_and(|ft| ft.is_dir()) {
                continue;
            }
            let path = entry.path();
            let manifest_path = path.join("manifest.json");
            let Ok(text) = fs::read_to_string(&manifest_path) else {
                continue;
            };
            let Ok(manifest) = serde_json::from_str::<Value>(&text) else {
                continue;
            };

            let mut page = settings_page(&manifest);
            if let Some(ref rel_page) = page {
                if !grant_page_access(&path, &manifest, rel_page) {
                    page = None;
                }
            }

            let name = display_name(&path, &manifest);
            let version = manifest
                .get("version")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string();
            let manifest_version = manifest
                .get("manifest_version")
                .and_then(Value::as_u64)
                .unwrap_or(2) as u8;
            let folder_name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();

            discovered.push(DiscoveredExtension {
                name,
                version,
                manifest_version,
                folder: folder_name,
                normalized_path: normalize(&path),
                path,
                settings_page: page,
            });
        }
    }

    discovered.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    if discovered.is_empty() {
        trace(&format!("extensions: none in {}", folder.display()));
    } else {
        for ext in &discovered {
            trace(&format!(
                "extensions: {} v{} MV{} [{}] options={}",
                ext.name,
                ext.version,
                ext.manifest_version,
                ext.folder,
                ext.settings_page.is_some()
            ));
        }
    }

    let profile_entries = read_profile_extensions(webview_dir);
    let wanted_paths: HashSet<&str> = discovered
        .iter()
        .map(|ext| ext.normalized_path.as_str())
        .collect();

    let mut ids_by_path = HashMap::new();
    let mut stale_ids = Vec::new();

    for (norm_path, entry) in &profile_entries {
        if wanted_paths.contains(norm_path.as_str()) {
            ids_by_path.insert(norm_path.clone(), entry.id.clone());
        } else if entry.is_unpacked {
            stale_ids.push(entry.id.clone());
        }
    }

    let mut missing_folders = Vec::new();
    for ext in &discovered {
        if !ids_by_path.contains_key(&ext.normalized_path) {
            missing_folders.push(ext.path.clone());
        }
    }

    if let Ok(mut reg) = registry().lock() {
        reg.discovered = discovered.clone();
        reg.ids_by_path = ids_by_path;
    }

    ProfileExtensionPlan {
        discovered,
        missing_folders,
        stale_ids,
    }
}

/// Build the list of embeddable extension settings pages using the in-memory
/// registry, falling back to reading profile preferences only if an ID is still unknown.
pub fn settings_pages(webview_dir: &Path, folder: &Path) -> Vec<ExtensionInfo> {
    let mut reg = registry().lock().unwrap_or_else(|e| e.into_inner());

    if reg.discovered.is_empty() && has_extensions(folder) {
        drop(reg);
        let _ = prepare_and_inspect(folder, webview_dir);
        reg = registry().lock().unwrap_or_else(|e| e.into_inner());
    }

    let needs_disk_lookup = reg
        .discovered
        .iter()
        .any(|ext| ext.settings_page.is_some() && !reg.ids_by_path.contains_key(&ext.normalized_path));

    if needs_disk_lookup {
        for (norm_path, entry) in read_profile_extensions(webview_dir) {
            reg.ids_by_path.entry(norm_path).or_insert(entry.id);
        }
    }

    let mut found = Vec::new();
    for ext in &reg.discovered {
        let Some(ref page) = ext.settings_page else {
            continue;
        };
        let Some(id) = reg.ids_by_path.get(&ext.normalized_path) else {
            continue;
        };
        found.push(ExtensionInfo {
            name: ext.name.clone(),
            url: format!("chrome-extension://{id}/{page}"),
        });
    }

    found.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    found.dedup_by(|a, b| a.url == b.url);
    found
}

/// Synchronize unpacked extensions in the live WebView2 profile once per session.
///
/// Only installs extensions that are not yet present in the profile (preventing
/// WebView2 from re-parsing and re-indexing large MV3 rulesets on every tab),
/// removes stale unpacked extensions, and records assigned extension IDs in memory.
#[cfg(windows)]
pub fn sync_webview_profile<F>(
    webview: &tauri::Webview,
    plan: ProfileExtensionPlan,
    on_complete: F,
) where
    F: FnOnce(bool) + Send + 'static,
{
    use webview2_com::{
        take_pwstr, BrowserExtensionEnableCompletedHandler, BrowserExtensionRemoveCompletedHandler,
        Microsoft::Web::WebView2::Win32::{ICoreWebView2Profile7, ICoreWebView2_13},
        ProfileAddBrowserExtensionCompletedHandler, ProfileGetBrowserExtensionsCompletedHandler,
    };
    use windows_core::{Interface, BOOL, HSTRING, PWSTR};

    let on_complete = Arc::new(Mutex::new(Some(on_complete)));
    let finish = {
        let on_complete = Arc::clone(&on_complete);
        move |added: bool| {
            if let Ok(mut guard) = on_complete.lock() {
                if let Some(callback) = guard.take() {
                    callback(added);
                }
            }
        }
    };

    let finish_outer = finish.clone();
    if webview
        .with_webview(move |platform_webview| {
            let finish_fallback = finish_outer.clone();
            let result = (|| -> windows_core::Result<()> {
                unsafe {
                    let core = platform_webview.controller().CoreWebView2()?;
                    let webview13: ICoreWebView2_13 = core.cast()?;
                    let profile = webview13.Profile()?;
                    let profile7: ICoreWebView2Profile7 = profile.cast()?;

                    let profile_for_add = profile7.clone();
                    let finish_inner = finish_outer.clone();

                    let get_handler = ProfileGetBrowserExtensionsCompletedHandler::create(Box::new(
                        move |error_code, list_opt| {
                            if error_code.is_err() {
                                finish_inner(false);
                                return Ok(());
                            }

                        let mut loaded_ids = HashSet::new();
                        let stale_set: HashSet<&str> =
                            plan.stale_ids.iter().map(String::as_str).collect();

                        if let Some(list) = list_opt {
                            let mut count = 0u32;
                            let _ = list.Count(&mut count);
                            for index in 0..count {
                                let Ok(ext) = list.GetValueAtIndex(index) else {
                                    continue;
                                };
                                let mut id_pwstr = PWSTR::null();
                                let id = if ext.Id(&mut id_pwstr).is_ok() {
                                    take_pwstr(id_pwstr)
                                } else {
                                    String::new()
                                };
                                let mut name_pwstr = PWSTR::null();
                                let name = if ext.Name(&mut name_pwstr).is_ok() {
                                    take_pwstr(name_pwstr)
                                } else {
                                    String::new()
                                };

                                if !id.is_empty() && stale_set.contains(id.as_str()) {
                                    trace(&format!("extensions: removing stale extension {id}"));
                                    let remove_handler =
                                        BrowserExtensionRemoveCompletedHandler::create(Box::new(
                                            |_| Ok(()),
                                        ));
                                    let _ = ext.Remove(&remove_handler);
                                    continue;
                                }

                                if !id.is_empty() {
                                    loaded_ids.insert(id.clone());
                                    if let Ok(mut reg) = registry().lock() {
                                        let matching_paths: Vec<String> = reg
                                            .discovered
                                            .iter()
                                            .filter(|item| item.name.eq_ignore_ascii_case(&name))
                                            .map(|item| item.normalized_path.clone())
                                            .collect();
                                        for path_key in matching_paths {
                                            reg.ids_by_path
                                                .entry(path_key)
                                                .or_insert_with(|| id.clone());
                                        }
                                    }
                                }

                                let mut enabled = BOOL::default();
                                if ext.IsEnabled(&mut enabled).is_ok() && !enabled.as_bool() {
                                    let enable_handler =
                                        BrowserExtensionEnableCompletedHandler::create(Box::new(
                                            |_| Ok(()),
                                        ));
                                    let _ = ext.Enable(true, &enable_handler);
                                }
                            }
                        }

                        // Determine which extensions in `.\extensions` need to be added to the profile.
                        let mut to_add: Vec<(PathBuf, String)> = Vec::new();
                        if let Ok(reg) = registry().lock() {
                            for item in &reg.discovered {
                                let already_loaded = reg
                                    .ids_by_path
                                    .get(&item.normalized_path)
                                    .is_some_and(|id| loaded_ids.contains(id));
                                if !already_loaded {
                                    to_add.push((item.path.clone(), item.normalized_path.clone()));
                                }
                            }
                        }

                        if to_add.is_empty() {
                            trace("extensions: profile already in sync");
                            finish_inner(false);
                            return Ok(());
                        }

                        let remaining = Arc::new(std::sync::atomic::AtomicUsize::new(to_add.len()));
                        let any_added = Arc::new(std::sync::atomic::AtomicBool::new(false));

                        for (folder_path, norm_path) in to_add {
                            let path_hs = HSTRING::from(folder_path.as_path());
                            let folder_for_cb = folder_path.clone();
                            let remaining_for_cb = Arc::clone(&remaining);
                            let any_added_for_cb = Arc::clone(&any_added);
                            let finish_add = finish_inner.clone();

                            let add_handler =
                                ProfileAddBrowserExtensionCompletedHandler::create(Box::new(
                                    move |add_res, ext_opt| {
                                        if add_res.is_ok() {
                                            if let Some(ext) = ext_opt {
                                                let mut id_pwstr = PWSTR::null();
                                                if ext.Id(&mut id_pwstr).is_ok() {
                                                    let id = take_pwstr(id_pwstr);
                                                    if !id.is_empty() {
                                                        trace(&format!(
                                                            "extensions: installed {} -> {id}",
                                                            folder_for_cb.display()
                                                        ));
                                                        if let Ok(mut reg) = registry().lock() {
                                                            reg.ids_by_path
                                                                .insert(norm_path.clone(), id);
                                                        }
                                                        any_added_for_cb.store(
                                                            true,
                                                            std::sync::atomic::Ordering::Release,
                                                        );
                                                    }
                                                }
                                            }
                                        } else {
                                            trace(&format!(
                                                "extensions: failed to install {}: {:?}",
                                                folder_for_cb.display(),
                                                add_res
                                            ));
                                        }

                                        if remaining_for_cb
                                            .fetch_sub(1, std::sync::atomic::Ordering::AcqRel)
                                            == 1
                                        {
                                            finish_add(
                                                any_added_for_cb
                                                    .load(std::sync::atomic::Ordering::Acquire),
                                            );
                                        }
                                        Ok(())
                                    },
                                ));

                            if let Err(err) =
                                profile_for_add.AddBrowserExtension(&path_hs, &add_handler)
                            {
                                trace(&format!(
                                    "extensions: AddBrowserExtension error for {}: {err}",
                                    folder_path.display()
                                ));
                                if remaining.fetch_sub(1, std::sync::atomic::Ordering::AcqRel) == 1
                                {
                                    finish_inner(
                                        any_added.load(std::sync::atomic::Ordering::Acquire),
                                    );
                                }
                            }
                        }

                        Ok(())
                    },
                ));

                profile7.GetBrowserExtensions(&get_handler)?;
                Ok(())
                }
            })();

            if let Err(err) = result {
                trace(&format!("extensions: profile sync error: {err}"));
                finish_fallback(false);
            }
        })
        .is_err()
    {
        finish(false);
    }
}

fn settings_page(manifest: &Value) -> Option<String> {
    let page = manifest
        .pointer("/options_ui/page")
        .and_then(Value::as_str)
        .or_else(|| manifest.get("options_page").and_then(Value::as_str))?;

    let page = page.trim_start_matches('/').trim();
    (!page.is_empty()).then(|| page.to_string())
}

fn display_name(folder: &Path, manifest: &Value) -> String {
    for key in ["name", "short_name"] {
        if let Some(raw) = manifest.get(key).and_then(Value::as_str) {
            if let Some(name) = resolve_message(folder, manifest, raw) {
                return name;
            }
        }
    }

    folder
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Extension".to_string())
}

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
    if !locales.iter().any(|l| l == "en") {
        locales.push("en".to_string());
    }
    if let Some(any) = first_locale(folder) {
        if !locales.contains(&any) {
            locales.push(any);
        }
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

fn first_locale(folder: &Path) -> Option<String> {
    fs::read_dir(folder.join("_locales"))
        .ok()?
        .flatten()
        .find(|entry| entry.file_type().is_ok_and(|ft| ft.is_dir()))
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
}

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

fn accessible_from_youtube(entry: &Map<String, Value>) -> bool {
    let Some(matches) = entry.get("matches").and_then(Value::as_array) else {
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
        host == "youtube.com" || host.ends_with(".youtube.com")
    })
}

fn covers(resource: &str, page: &str) -> bool {
    glob_matches(
        resource.trim_start_matches('/'),
        page.trim_start_matches('/'),
    )
}

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

struct ProfileExtensionEntry {
    id: String,
    is_unpacked: bool,
}

fn read_profile_extensions(webview_dir: &Path) -> HashMap<String, ProfileExtensionEntry> {
    let mut entries = HashMap::new();

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
            let Some(folder) = value.get("path").and_then(Value::as_str) else {
                continue;
            };
            let norm = normalize(Path::new(folder));
            let location = value.get("location").and_then(Value::as_u64).unwrap_or(0);
            let is_component =
                location == 5 || norm.contains("\\microsoft\\edgewebview\\application\\");
            entries.insert(
                norm,
                ProfileExtensionEntry {
                    id: id.clone(),
                    is_unpacked: location == 4 && !is_component,
                },
            );
        }
    }

    entries
}

pub fn normalize(path: &Path) -> String {
    let raw = path.to_string_lossy();
    let stripped = raw.strip_prefix(r"\\?\").unwrap_or(&raw);
    stripped
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase()
}

pub fn has_extensions(folder: &Path) -> bool {
    fs::read_dir(folder)
        .map(|entries| {
            entries.flatten().any(|entry| {
                entry.file_type().is_ok_and(|ft| ft.is_dir())
                    && entry.path().join("manifest.json").is_file()
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
