//! Tabbed browsing shell for YouTube Desktop.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{
    menu::{Menu, MenuItem},
    webview::{Color, NewWindowResponse, WebviewBuilder},
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, Position, Rect, Size, Url,
    Webview, WebviewUrl, WindowEvent,
};
use tauri_plugin_opener::OpenerExt;

use crate::app::extensions::{self, ExtensionInfo, ProfileExtensionPlan};
use crate::app::profile::{
    extensions_disabled, requested as requested_profile, resolve as resolve_profile,
    DEFAULT_PROFILE,
};
use crate::app::youtube;

pub const WINDOW_LABEL: &str = "pake";
pub const CHROME_LABEL: &str = "pake-chrome";
const CHROME_HEIGHT: f64 = 40.0;
const DEFAULT_WIDTH: f64 = 1440.0;
const DEFAULT_HEIGHT: f64 = 900.0;
const APP_IDENTIFIER: &str = "com.pake.youtube.tabs";
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/147.0.0.0 Safari/537.36";

const BRIDGE_JS: &str = include_str!("../inject/tab_bridge.js");
const NEW_TAB_TITLE: &str = "New tab";
const EXTENSION_MENU_PREFIX: &str = "pake-ext-";
const STALL_RETRY: Duration = Duration::from_secs(4);
const STALL_ATTEMPTS: u32 = 5;

static FULLSCREEN: AtomicBool = AtomicBool::new(false);
static EXTENSION_LOAD_FAILED: AtomicBool = AtomicBool::new(false);
static PROFILE_SYNCED: AtomicBool = AtomicBool::new(false);
static LAST_POPUP: Mutex<Option<(String, Instant)>> = Mutex::new(None);
const POPUP_REPEAT: Duration = Duration::from_millis(700);

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TabView {
    pub id: u32,
    pub title: String,
    pub url: String,
    pub loading: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TabsState {
    pub tabs: Vec<TabView>,
    pub active: Option<u32>,
    pub extensions: Vec<ExtensionInfo>,
}

pub struct TabEntry {
    pub id: u32,
    pub title: String,
    pub url: String,
    pub loading: bool,
    started: bool,
}

struct Pending {
    target: Url,
    dispatched: bool,
}

#[derive(Default)]
struct Tabs {
    items: Vec<TabEntry>,
    active: Option<u32>,
    next_id: u32,
    pending: HashMap<u32, Pending>,
}

pub struct TabManager {
    state: Mutex<Tabs>,
    pub data_dir: PathBuf,
    pub session_path: Option<PathBuf>,
    pub extensions_dir: Option<PathBuf>,
    pub extension_plan: ProfileExtensionPlan,
    pub profile_id: u32,
    extensions: Mutex<Vec<ExtensionInfo>>,
}

impl TabManager {
    pub fn new(
        data_dir: PathBuf,
        session_path: Option<PathBuf>,
        extensions_dir: Option<PathBuf>,
        extension_plan: ProfileExtensionPlan,
        profile_id: u32,
        initial_extensions: Vec<ExtensionInfo>,
    ) -> Self {
        Self {
            state: Mutex::new(Tabs::default()),
            data_dir,
            session_path,
            extensions_dir,
            extension_plan,
            profile_id,
            extensions: Mutex::new(initial_extensions),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Tabs> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }

    fn refresh_extensions(&self) -> Vec<ExtensionInfo> {
        let found = self
            .extensions_dir
            .as_deref()
            .map(|folder| extensions::settings_pages(&self.data_dir, folder))
            .unwrap_or_default();

        let mut cache = self
            .extensions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if *cache != found {
            *cache = found.clone();
        }
        found
    }

    fn extension_at(&self, index: usize) -> Option<ExtensionInfo> {
        let cache = self
            .extensions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        cache.get(index).cloned()
    }

    fn snapshot(&self) -> TabsState {
        let state = self.lock();
        TabsState {
            tabs: state
                .items
                .iter()
                .map(|tab| TabView {
                    id: tab.id,
                    title: tab.title.clone(),
                    url: tab.url.clone(),
                    loading: tab.loading,
                })
                .collect(),
            active: state.active,
            extensions: self
                .extensions
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .clone(),
        }
    }

    fn active_id(&self) -> Option<u32> {
        self.lock().active
    }
}

pub fn trace(message: &str) {
    use std::io::Write;

    static START: OnceLock<Instant> = OnceLock::new();
    let elapsed = START.get_or_init(Instant::now).elapsed().as_millis();

    let path = std::env::temp_dir().join("ytpake-trace.log");
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "[{elapsed:>6}ms] {message}");
    }
}

#[cfg(windows)]
fn set_app_user_model_id(id: &str) {
    use windows_sys::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;
    let wide: Vec<u16> = id.encode_utf16().chain(std::iter::once(0)).collect();
    let _ = unsafe { SetCurrentProcessExplicitAppUserModelID(wide.as_ptr()) };
}

fn tab_label(id: u32) -> String {
    format!("pake-tab-{id}")
}

fn parse_tab_id(label: &str) -> Option<u32> {
    label.strip_prefix("pake-tab-")?.parse().ok()
}

fn open_external(app: &AppHandle, url: &Url) {
    if !matches!(url.scheme(), "http" | "https") {
        return;
    }
    let handle = app.clone();
    let href = url.to_string();
    std::thread::spawn(move || {
        trace(&format!(
            "link: {href} is outside YouTube, opening externally"
        ));
        if let Err(error) = handle.opener().open_url(href.as_str(), None::<&str>) {
            trace(&format!("link: could not open {href}: {error}"));
        }
    });
}

pub fn layout(app: &AppHandle) {
    let Some(window) = app.get_window(WINDOW_LABEL) else {
        return;
    };
    let (Ok(size), Ok(scale)) = (window.inner_size(), window.scale_factor()) else {
        return;
    };

    let fullscreen = FULLSCREEN.load(Ordering::Relaxed);
    let strip = if fullscreen {
        0
    } else {
        ((CHROME_HEIGHT * scale).round() as u32).min(size.height)
    };
    let content_height = size.height.saturating_sub(strip);

    if let Some(chrome) = app.get_webview(CHROME_LABEL) {
        set_bounds(&chrome, 0, 0, size.width, strip.max(1));
        if fullscreen {
            let _ = chrome.hide();
        } else {
            let _ = chrome.show();
        }
    }
    for webview in app.webviews().values() {
        if webview.label() == CHROME_LABEL {
            continue;
        }
        set_bounds(&webview, 0, strip as i32, size.width, content_height);
    }
}

fn set_bounds(webview: &Webview, x: i32, y: i32, width: u32, height: u32) {
    let _ = webview.set_bounds(Rect {
        position: Position::Physical(PhysicalPosition::new(x, y)),
        size: Size::Physical(PhysicalSize::new(width, height)),
    });
}

fn emit_state(app: &AppHandle) {
    let manager = app.state::<TabManager>();
    let snapshot = manager.snapshot();
    if let Some(chrome) = app.get_webview(CHROME_LABEL) {
        let _ = chrome.emit("tabs:update", snapshot);
    }
}

fn apply_active(app: &AppHandle) {
    let manager = app.state::<TabManager>();
    let active = manager.active_id();
    let warming: Vec<u32> = manager.lock().pending.keys().copied().collect();
    layout(app);
    for webview in app.webviews().values() {
        if webview.label() == CHROME_LABEL {
            continue;
        }
        let id = parse_tab_id(webview.label());
        if id == active && !id.is_some_and(|id| warming.contains(&id)) {
            let _ = webview.show();
            let _ = webview.set_focus();
            if let Some(id) = id {
                resume_target(app, id, &webview);
            }
        } else {
            let _ = webview.hide();
        }
    }
}

fn resume_target(app: &AppHandle, id: u32, webview: &Webview) {
    let manager = app.state::<TabManager>();
    let target = {
        let state = manager.lock();
        match state.items.iter().find(|tab| tab.id == id) {
            Some(tab) if !tab.started => tab.url.clone(),
            _ => return,
        }
    };
    let current = webview.url().map(|url| url.to_string()).unwrap_or_default();
    if current == target {
        return;
    }
    let Ok(url) = Url::parse(&target) else {
        return;
    };
    trace(&format!(
        "tab {id}: shown without a page, resuming at the destination"
    ));
    let _ = webview.navigate(url);
}

fn build_webview(
    app: &AppHandle,
    id: u32,
    url: Url,
    strip: u32,
    width: u32,
    height: u32,
    allow_extensions: bool,
) -> tauri::Result<Webview> {
    let manager = app.state::<TabManager>();
    let label = tab_label(id);
    let data_dir = manager.data_dir.clone();
    let use_extensions = allow_extensions
        && !extensions_disabled()
        && !EXTENSION_LOAD_FAILED.load(Ordering::Relaxed)
        && manager.extension_plan.has_extensions();

    let need_sync = use_extensions && !PROFILE_SYNCED.swap(true, Ordering::AcqRel);
    let wait_for_initial_install =
        need_sync && !manager.extension_plan.missing_folders.is_empty();

    let start = if wait_for_initial_install {
        manager.lock().pending.insert(
            id,
            Pending {
                target: url.clone(),
                dispatched: false,
            },
        );
        Url::parse("about:blank").unwrap_or(url)
    } else {
        manager.lock().pending.remove(&id);
        url
    };

    let title_app = app.clone();
    let load_app = app.clone();
    let popup_app = app.clone();
    let navigation_app = app.clone();

    let mut builder = WebviewBuilder::new(label, WebviewUrl::External(start))
        .user_agent(USER_AGENT)
        .data_directory(data_dir)
        .background_color(Color(15, 15, 15, 255))
        .initialization_script(BRIDGE_JS)
        .on_navigation(move |url| {
            if youtube::allows_navigation(url) || url.as_str() == "about:blank" {
                return true;
            }
            open_external(&navigation_app, url);
            false
        })
        .on_new_window(move |url, _features| {
            if !matches!(url.scheme(), "http" | "https") {
                return NewWindowResponse::Deny;
            }
            if !youtube::is_youtube_target(&url) {
                open_external(&popup_app, &url);
                return NewWindowResponse::Deny;
            }
            let target = url.to_string();
            if !popup_is_duplicate(&target) {
                let app = popup_app.clone();
                std::thread::spawn(move || {
                    let _ = open_tab(&app, Some(target), false);
                });
            }
            NewWindowResponse::Deny
        })
        .on_document_title_changed(move |webview, title| {
            let trimmed = title.trim();
            if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("about:blank") {
                return;
            }
            if let Ok(current) = webview.url() {
                if current.as_str() == "about:blank" {
                    return;
                }
            }
            if let Some(id) = parse_tab_id(webview.label()) {
                update_title(&title_app, id, trimmed);
            }
        })
        .on_page_load(move |webview, payload| {
            if payload.url().as_str() == "about:blank" {
                return;
            }
            if let Some(id) = parse_tab_id(webview.label()) {
                let finished = matches!(payload.event(), tauri::webview::PageLoadEvent::Finished);
                update_loading(&load_app, id, payload.url(), finished);
            }
        });

    if use_extensions {
        builder = builder.browser_extensions_enabled(true);
    }

    #[cfg(target_os = "windows")]
    {
        builder = builder.additional_browser_args(
            "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection \
             --autoplay-policy=no-user-gesture-required",
        );
    }

    let window = app
        .get_window(WINDOW_LABEL)
        .ok_or_else(|| tauri::Error::WindowNotFound)?;

    let webview = window.add_child(
        builder,
        PhysicalPosition::new(0, strip as i32),
        PhysicalSize::new(width, height),
    )?;
    trace(&format!("tab {id}: webview added"));

    if wait_for_initial_install {
        let _ = webview.hide();
    }

    #[cfg(windows)]
    if need_sync {
        let sync_app = app.clone();
        let plan = manager.extension_plan.clone();
        extensions::sync_webview_profile(&webview, plan, move |added_any| {
            let manager = sync_app.state::<TabManager>();
            if !manager.refresh_extensions().is_empty() {
                emit_state(&sync_app);
            }
            if wait_for_initial_install {
                send_to_target(&sync_app, id);
            } else if added_any {
                if let Some(wv) = sync_app.get_webview(&tab_label(id)) {
                    let _ = wv.reload();
                }
            }
        });
    }

    Ok(webview)
}

fn popup_is_duplicate(target: &str) -> bool {
    let mut last = LAST_POPUP.lock().unwrap_or_else(|error| error.into_inner());
    if let Some((seen, at)) = last.as_ref() {
        if seen == target && at.elapsed() < POPUP_REPEAT {
            return true;
        }
    }
    *last = Some((target.to_string(), Instant::now()));
    false
}

fn open_extension_settings(app: &AppHandle, url: &str) -> Result<(), String> {
    if !url.starts_with("chrome-extension://") {
        return Err("invalid extension URL".into());
    }
    let webview = active_webview(app).ok_or_else(|| "no active tab".to_string())?;
    let payload = serde_json::to_string(url).map_err(|error| error.to_string())?;
    webview
        .eval(&format!(
            "window.__pakeOpenSettings && window.__pakeOpenSettings({payload})"
        ))
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn tabs_extension_menu(app: AppHandle) -> Result<(), String> {
    let manager = app.state::<TabManager>();
    let found = manager.refresh_extensions();
    if found.is_empty() {
        return Err("no extension with a settings page".into());
    }

    if let [only] = found.as_slice() {
        return open_extension_settings(&app, &only.url);
    }

    let menu = Menu::new(&app).map_err(|error| error.to_string())?;
    for (index, info) in found.iter().enumerate() {
        let item = MenuItem::with_id(
            &app,
            format!("{EXTENSION_MENU_PREFIX}{index}"),
            &info.name,
            true,
            None::<&str>,
        )
        .map_err(|error| error.to_string())?;
        menu.append(&item).map_err(|error| error.to_string())?;
    }

    let window = app
        .get_window(WINDOW_LABEL)
        .ok_or_else(|| "window unavailable".to_string())?;
    window.popup_menu(&menu).map_err(|error| error.to_string())
}

fn reserve_tab(app: &AppHandle, url: Option<&str>, activate: bool) -> (u32, Url) {
    let manager = app.state::<TabManager>();
    let target = youtube::target(url.unwrap_or(youtube::HOME));

    let id = {
        let mut state = manager.lock();
        let id = state.next_id;
        state.next_id += 1;
        state.items.push(TabEntry {
            id,
            title: NEW_TAB_TITLE.to_string(),
            url: target.to_string(),
            loading: true,
            started: false,
        });
        if activate {
            state.active = Some(id);
        }
        id
    };

    (id, target)
}

fn create_webview(app: &AppHandle, id: u32, target: Url) -> Result<(), String> {
    let manager = app.state::<TabManager>();
    let (width, height, strip) = window_metrics(app);

    let mut built = build_webview(app, id, target.clone(), strip, width, height, true);
    if built.is_err() && !extensions_disabled() {
        trace("extension loading failed; opening tabs without extensions for this session");
        EXTENSION_LOAD_FAILED.store(true, Ordering::Relaxed);
        built = build_webview(app, id, target.clone(), strip, width, height, false);
    }
    if let Err(error) = built {
        trace(&format!("failed to open a tab at {target}: {error}"));
        let mut state = manager.lock();
        state.items.retain(|tab| tab.id != id);
        state.pending.remove(&id);
        if state.active == Some(id) {
            state.active = state.items.first().map(|tab| tab.id);
        }
        return Err(error.to_string());
    }

    apply_active(app);
    emit_state(app);
    save_session(app);
    watch_for_stall(app.clone(), id, target);
    Ok(())
}

pub fn open_tab(app: &AppHandle, url: Option<String>, activate: bool) -> Result<u32, String> {
    let (id, target) = reserve_tab(app, url.as_deref(), activate);
    create_webview(app, id, target)?;
    Ok(id)
}

fn watch_for_stall(app: AppHandle, id: u32, target: Url) {
    tauri::async_runtime::spawn(async move {
        for attempt in 1..=STALL_ATTEMPTS {
            tokio::time::sleep(STALL_RETRY).await;

            let manager = app.state::<TabManager>();
            let waiting = {
                let state = manager.lock();
                state.items.iter().any(|tab| tab.id == id && !tab.started)
            };
            if !waiting {
                return;
            }
            trace(&format!("tab {id}: still not started, retry {attempt}"));
            if let Some(webview) = app.get_webview(&tab_label(id)) {
                let _ = webview.navigate(target.clone());
            }
        }
    });
}

fn window_metrics(app: &AppHandle) -> (u32, u32, u32) {
    let Some(window) = app.get_window(WINDOW_LABEL) else {
        return (1200, 800, 40);
    };
    let size = window.inner_size().unwrap_or(PhysicalSize::new(1200, 800));
    let scale = window.scale_factor().unwrap_or(1.0);
    let strip = if FULLSCREEN.load(Ordering::Relaxed) {
        0
    } else {
        ((CHROME_HEIGHT * scale).round() as u32).min(size.height)
    };
    (size.width, size.height.saturating_sub(strip), strip)
}

fn set_window_title(app: &AppHandle, display: &str) {
    let profile_id = app.state::<TabManager>().profile_id;
    let Some(window) = app.get_window(WINDOW_LABEL) else {
        return;
    };
    let mut title = if display.eq_ignore_ascii_case("youtube") {
        display.to_string()
    } else {
        format!("{display} - YouTube")
    };
    if profile_id != DEFAULT_PROFILE {
        title.push_str(&format!(" (profile {profile_id})"));
    }
    let _ = window.set_title(&title);
}

fn refresh_window_title(app: &AppHandle) {
    let title = {
        let manager = app.state::<TabManager>();
        let state = manager.lock();
        state
            .active
            .and_then(|id| state.items.iter().find(|tab| tab.id == id))
            .map(|tab| tab.title.clone())
    };
    if let Some(title) = title {
        set_window_title(app, &title);
    }
}

fn update_title(app: &AppHandle, id: u32, title: &str) {
    let cleaned = title
        .trim()
        .strip_suffix(" - YouTube")
        .unwrap_or(title.trim())
        .to_string();
    let display = if cleaned.is_empty() {
        NEW_TAB_TITLE.to_string()
    } else {
        cleaned
    };

    let is_active = {
        let manager = app.state::<TabManager>();
        let mut state = manager.lock();
        let Some(tab) = state.items.iter_mut().find(|tab| tab.id == id) else {
            return;
        };
        if tab.title == display {
            return;
        }
        tab.title = display.clone();
        state.active == Some(id)
    };

    if is_active {
        set_window_title(app, &display);
    }
    emit_state(app);
}

fn update_loading(app: &AppHandle, id: u32, url: &Url, finished: bool) {
    let manager = app.state::<TabManager>();
    let mut reveal = false;
    {
        let mut state = manager.lock();
        if state.pending.remove(&id).is_some() {
            reveal = true;
        }
        if let Some(tab) = state.items.iter_mut().find(|tab| tab.id == id) {
            tab.loading = !finished;
            tab.started = true;
            tab.url = url.to_string();
        } else {
            return;
        }
    }
    if reveal {
        apply_active(app);
    }
    emit_state(app);
    if finished {
        save_session(app);
        if manager.snapshot().extensions.is_empty() && !manager.refresh_extensions().is_empty() {
            emit_state(app);
        }
    }
}

fn send_to_target(app: &AppHandle, id: u32) {
    let manager = app.state::<TabManager>();
    let target = {
        let mut state = manager.lock();
        let Some(entry) = state.pending.get_mut(&id) else {
            return;
        };
        if entry.dispatched {
            return;
        }
        entry.dispatched = true;
        entry.target.clone()
    };

    if let Some(webview) = app.get_webview(&tab_label(id)) {
        trace(&format!("tab {id}: navigating to {target}"));
        let _ = webview.navigate(target);
    }
}

fn ensure_webview(app: &AppHandle, id: u32) -> bool {
    if app.get_webview(&tab_label(id)).is_some() {
        return false;
    }

    let target = {
        let manager = app.state::<TabManager>();
        let state = manager.lock();
        match state.items.iter().find(|tab| tab.id == id) {
            Some(tab) => youtube::target(&tab.url),
            None => return false,
        }
    };

    create_webview(app, id, target).is_ok()
}

pub fn activate_tab(app: &AppHandle, id: u32) -> Result<(), String> {
    let manager = app.state::<TabManager>();
    let previous = {
        let mut state = manager.lock();
        if !state.items.iter().any(|tab| tab.id == id) {
            return Err("tab not found".into());
        }
        if state.active == Some(id) {
            drop(state);
            ensure_webview(app, id);
            apply_active(app);
            return Ok(());
        }
        let previous = state.active;
        state.active = Some(id);
        previous
    };

    if FULLSCREEN.swap(false, Ordering::Relaxed) {
        if let Some(previous) = previous.and_then(|id| app.get_webview(&tab_label(id))) {
            let _ = previous.eval("document.fullscreenElement && document.exitFullscreen()");
        }
    }

    ensure_webview(app, id);

    apply_active(app);
    refresh_window_title(app);
    emit_state(app);
    save_session(app);
    Ok(())
}

pub fn close_tab(app: &AppHandle, id: u32) -> Result<(), String> {
    let manager = app.state::<TabManager>();
    let (remaining, was_active) = {
        let mut state = manager.lock();
        let index = state
            .items
            .iter()
            .position(|tab| tab.id == id)
            .ok_or_else(|| "tab not found".to_string())?;
        state.items.remove(index);
        state.pending.remove(&id);
        let was_active = state.active == Some(id);
        if was_active {
            let next = index
                .saturating_sub(1)
                .min(state.items.len().saturating_sub(1));
            state.active = state.items.get(next).map(|tab| tab.id);
        }
        (state.items.len(), was_active)
    };

    if let Some(webview) = app.get_webview(&tab_label(id)) {
        let _ = webview.close();
    }
    if was_active && FULLSCREEN.swap(false, Ordering::Relaxed) {
        layout(app);
    }

    if remaining == 0 {
        if let Some(window) = app.get_window(WINDOW_LABEL) {
            let _ = window.close();
        }
        return Ok(());
    }

    if was_active {
        if let Some(next) = manager.active_id() {
            ensure_webview(app, next);
        }
        apply_active(app);
        refresh_window_title(app);
    }
    emit_state(app);
    save_session(app);
    Ok(())
}

pub fn cycle_tab(app: &AppHandle, delta: i32) -> Result<(), String> {
    let manager = app.state::<TabManager>();
    let next = {
        let state = manager.lock();
        if state.items.is_empty() {
            return Err("no tabs".into());
        }
        let current = state
            .items
            .iter()
            .position(|tab| Some(tab.id) == state.active)
            .unwrap_or(0) as i32;
        let len = state.items.len() as i32;
        let index = (current + delta).rem_euclid(len) as usize;
        state.items[index].id
    };
    activate_tab(app, next)
}

pub fn select_index(app: &AppHandle, index: i32) -> Result<(), String> {
    let manager = app.state::<TabManager>();
    let target = {
        let state = manager.lock();
        if state.items.is_empty() {
            return Err("no tabs".into());
        }
        let position = if index < 0 {
            state.items.len() - 1
        } else {
            (index as usize).min(state.items.len() - 1)
        };
        state.items[position].id
    };
    activate_tab(app, target)
}

fn active_webview(app: &AppHandle) -> Option<Webview> {
    let id = app.state::<TabManager>().active_id()?;
    app.get_webview(&tab_label(id))
}

#[derive(Serialize, Deserialize, Default)]
struct Session {
    tabs: Vec<String>,
    #[serde(default)]
    titles: Vec<String>,
    #[serde(default)]
    active: usize,
}

fn load_session(path: &Option<PathBuf>) -> Option<Session> {
    let path = path.as_ref()?;
    let raw = fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

fn save_session(app: &AppHandle) {
    let manager = app.state::<TabManager>();
    let Some(path) = manager.session_path.clone() else {
        return;
    };

    let (entries, active) = {
        let state = manager.lock();
        let active = state
            .items
            .iter()
            .position(|tab| Some(tab.id) == state.active)
            .unwrap_or(0);
        let entries: Vec<(u32, String, String)> = state
            .items
            .iter()
            .map(|tab| (tab.id, tab.url.clone(), tab.title.clone()))
            .collect();
        (entries, active)
    };

    let mut tabs: Vec<String> = Vec::with_capacity(entries.len());
    let mut titles: Vec<String> = Vec::with_capacity(entries.len());
    for (id, fallback, title) in entries {
        let url = app
            .get_webview(&tab_label(id))
            .and_then(|webview| webview.url().ok())
            .filter(|url| url.as_str() != "about:blank")
            .map(|url| url.to_string())
            .unwrap_or(fallback);
        tabs.push(url);
        titles.push(title);
    }

    if tabs.is_empty() {
        return;
    }

    let session = Session {
        tabs,
        titles,
        active,
    };
    if let Ok(raw) = serde_json::to_string_pretty(&session) {
        let _ = fs::write(path, raw);
    }
}

#[tauri::command]
pub async fn tabs_new(app: AppHandle, url: Option<String>) -> Result<(), String> {
    trace("tab: new requested");
    open_tab(&app, url, true).map(|_| ())
}

#[tauri::command]
pub async fn tabs_close(app: AppHandle, id: u32) -> Result<(), String> {
    close_tab(&app, id)
}

#[tauri::command]
pub async fn tabs_close_active(app: AppHandle) -> Result<(), String> {
    let id = app
        .state::<TabManager>()
        .active_id()
        .ok_or_else(|| "no active tab".to_string())?;
    close_tab(&app, id)
}

#[tauri::command]
pub async fn tabs_activate(app: AppHandle, id: u32) -> Result<(), String> {
    activate_tab(&app, id)
}

#[tauri::command]
pub async fn tabs_select(app: AppHandle, index: i32) -> Result<(), String> {
    select_index(&app, index)
}

#[tauri::command]
pub async fn tabs_cycle(app: AppHandle, delta: i32) -> Result<(), String> {
    cycle_tab(&app, delta)
}

#[tauri::command]
pub async fn tabs_navigate(app: AppHandle, url: String) -> Result<(), String> {
    let target = youtube::target(&url);
    let webview = active_webview(&app).ok_or_else(|| "no active tab".to_string())?;
    webview.navigate(target).map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn tabs_open_external(app: AppHandle, url: String) -> Result<(), String> {
    let target = Url::parse(&url).map_err(|error| error.to_string())?;
    if !matches!(target.scheme(), "http" | "https") || youtube::allows_navigation(&target) {
        return Err("external link expected".into());
    }
    app.opener()
        .open_url(target.as_str(), None::<&str>)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn tabs_reload(app: AppHandle) -> Result<(), String> {
    let webview = active_webview(&app).ok_or_else(|| "no active tab".to_string())?;
    webview.reload().map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn tabs_back(app: AppHandle) -> Result<(), String> {
    let webview = active_webview(&app).ok_or_else(|| "no active tab".to_string())?;
    webview
        .eval("history.back()")
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn tabs_forward(app: AppHandle) -> Result<(), String> {
    let webview = active_webview(&app).ok_or_else(|| "no active tab".to_string())?;
    webview
        .eval("history.forward()")
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn tabs_fullscreen(app: AppHandle, active: bool) -> Result<(), String> {
    FULLSCREEN.store(active, Ordering::Relaxed);
    apply_active(&app);
    Ok(())
}

#[tauri::command]
pub async fn tabs_state(app: AppHandle) -> Result<TabsState, String> {
    Ok(app.state::<TabManager>().snapshot())
}

fn prepare_session_tabs(app: &AppHandle) -> u32 {
    let manager = app.state::<TabManager>();
    let session_path = manager.session_path.clone();

    let Some(session) = load_session(&session_path).filter(|s| !s.tabs.is_empty()) else {
        let (id, _) = reserve_tab(app, None, true);
        return id;
    };

    let active = session.active.min(session.tabs.len().saturating_sub(1));
    let mut reserved = Vec::with_capacity(session.tabs.len());
    for url in &session.tabs {
        reserved.push(reserve_tab(app, Some(url), false).0);
    }

    let active_id = reserved.get(active).copied().unwrap_or(0);
    {
        let mut state = manager.lock();
        state.active = Some(active_id);
        for (index, tab) in state.items.iter_mut().enumerate() {
            tab.loading = tab.id == active_id;
            if let Some(title) = session.titles.get(index) {
                if !title.trim().is_empty() {
                    tab.title = title.clone();
                }
            }
        }
    }
    active_id
}

fn create_shell(app: &AppHandle) -> tauri::Result<()> {
    trace("shell: start");
    let manager = app.state::<TabManager>();

    let title = if manager.profile_id == DEFAULT_PROFILE {
        "YouTube".to_string()
    } else {
        format!("YouTube (profile {})", manager.profile_id)
    };

    let active_tab_id = prepare_session_tabs(app);

    let window = tauri::window::WindowBuilder::new(app, WINDOW_LABEL)
        .title(title)
        .inner_size(DEFAULT_WIDTH, DEFAULT_HEIGHT)
        .min_inner_size(560.0, 400.0)
        .resizable(true)
        .center()
        .visible(false)
        .background_color(tauri::window::Color(15, 15, 15, 255))
        .build()?;

    if let Some(icon) = app.default_window_icon().cloned() {
        let _ = window.set_icon(icon);
    }

    let size = window.inner_size()?;

    let _ = window.show();
    let _ = window.set_focus();
    refresh_window_title(app);
    trace("shell: shown");

    let shell = app.clone();
    tauri::async_runtime::spawn(async move {
        create_strip(&shell, size);
        let handle = shell.clone();
        std::thread::spawn(move || {
            ensure_webview(&handle, active_tab_id);
            layout(&handle);
            emit_state(&handle);
            trace("tabs: restored");
        });
    });

    Ok(())
}

fn create_strip(app: &AppHandle, size: PhysicalSize<u32>) {
    let Some(window) = app.get_window(WINDOW_LABEL) else {
        return;
    };
    let scale = window.scale_factor().unwrap_or(1.0);
    let strip = ((CHROME_HEIGHT * scale).round() as u32).min(size.height);

    match window.add_child(
        WebviewBuilder::new(CHROME_LABEL, WebviewUrl::App("tabs.html".into()))
            .initialization_script("window.__PAKE_TAB_SHELL__ = true;"),
        PhysicalPosition::new(0, 0),
        PhysicalSize::new(size.width, strip),
    ) {
        Ok(chrome) => {
            let _ = chrome.set_focus();
            trace("strip: ready");
        }
        Err(error) => trace(&format!("strip: {error}")),
    }
}

pub fn run_tabbed() {
    let profile_id = requested_profile();
    let identifier = match profile_id {
        Some(id) => format!("{APP_IDENTIFIER}.profile{id}"),
        None => APP_IDENTIFIER.to_string(),
    };

    #[cfg(windows)]
    set_app_user_model_id(&identifier);

    let context = tauri::generate_context!();

    let mut builder = tauri::Builder::default();

    if profile_id.is_none() {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_window(WINDOW_LABEL) {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
        }));
    }

    let active_profile = profile_id.unwrap_or(DEFAULT_PROFILE);
    let window_state = tauri_plugin_window_state::Builder::default()
        .with_state_flags(
            tauri_plugin_window_state::StateFlags::SIZE
                | tauri_plugin_window_state::StateFlags::POSITION
                | tauri_plugin_window_state::StateFlags::MAXIMIZED,
        )
        .with_filename(format!(".window-state-profile{active_profile}.json"));

    let builder = builder
        .plugin(tauri_plugin_opener::init())
        .plugin(window_state.build())
        .invoke_handler(tauri::generate_handler![
            tabs_new,
            tabs_close,
            tabs_close_active,
            tabs_activate,
            tabs_select,
            tabs_cycle,
            tabs_navigate,
            tabs_reload,
            tabs_back,
            tabs_forward,
            tabs_state,
            tabs_fullscreen,
            tabs_extension_menu,
            tabs_open_external,
        ])
        .on_menu_event(|app, event| {
            let id = event.id().0.clone();
            let Some(index) = id
                .strip_prefix(EXTENSION_MENU_PREFIX)
                .and_then(|rest| rest.parse::<usize>().ok())
            else {
                return;
            };
            let target = app.state::<TabManager>().extension_at(index);
            if let Some(info) = target {
                let _ = open_extension_settings(app, &info.url);
            }
        })
        .setup(move |app| {
            trace("setup: start");
            let handle = app.app_handle().clone();
            let app_root = handle.path().app_data_dir().ok();
            let profile = resolve_profile(app_root.as_deref());
            let data_dir = profile.webview_dir();

            let plan = if extensions_disabled() {
                ProfileExtensionPlan::default()
            } else {
                extensions::prepare_and_inspect(&profile.extensions, &data_dir)
            };

            trace(&format!(
                "setup: extensions folder {}",
                profile.extensions.display()
            ));

            let initial_extensions = if extensions_disabled() {
                Vec::new()
            } else {
                extensions::settings_pages(&data_dir, &profile.extensions)
            };

            let session_path = Some(profile.session_path());
            let extensions_dir = Some(profile.extensions);

            app.manage(TabManager::new(
                data_dir,
                session_path,
                extensions_dir,
                plan,
                profile.id,
                initial_extensions,
            ));

            create_shell(&handle)?;
            Ok(())
        })
        .on_window_event(|window, event| match event {
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                let app = window.app_handle().clone();
                layout(&app);
            }
            WindowEvent::CloseRequested { .. } => {
                let app = window.app_handle().clone();
                save_session(&app);
            }
            _ => {}
        });

    builder
        .build(context)
        .unwrap_or_else(|error| {
            eprintln!("[Pake] Fatal error while building the tabbed app: {error}");
            std::process::exit(1);
        })
        .run(|_app, _event| {});
}
