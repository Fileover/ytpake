//! Tabbed browsing shell.
//!
//! Pake's default shell is one webview per window. That is fine for a single
//! page, but a site like YouTube is used across several pages at once. This
//! module keeps a single native window and hosts one child webview per tab,
//! plus a small local webview at the top that draws the tab strip.
//!
//! The content webviews are real WebView2/WKWebView instances, so every tab
//! keeps its own player, scroll position and history; switching tabs only
//! toggles their visibility.

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
    AppHandle, Config, Emitter, Manager, PhysicalPosition, PhysicalSize, Position, Rect, Size, Url,
    Webview, WebviewUrl, WindowEvent,
};
use tauri_plugin_opener::OpenerExt;

use crate::app::config::PakeConfig;
use crate::app::extensions::{self, ExtensionInfo};
use crate::app::filters;
use crate::app::profile::{
    adblock_enabled, extensions_disabled, requested as requested_profile,
    resolve as resolve_profile, DEFAULT_PROFILE,
};
use crate::app::youtube;

/// Label of the native window that hosts every webview.
pub const WINDOW_LABEL: &str = "pake";
/// Label of the local webview that draws the tab strip.
pub const CHROME_LABEL: &str = "pake-chrome";
/// Height of the tab strip, in logical pixels.
const CHROME_HEIGHT: f64 = 40.0;
/// Injected into every content webview before the page's own scripts run.
const BRIDGE_JS: &str = include_str!("../inject/tab_bridge.js");
const ADBLOCK_JS: &str = include_str!("../inject/adblock.js");

/// The path of the throwaway document a tab shows before its real address.
/// A 404 is the cheapest HTML any site will serve, and it is never the address
/// the tab was asked to open.
const WARMUP_PATH: &str = "/this-page-does-not-exist-warmup";
const NEW_TAB_TITLE: &str = "Nova aba";
/// Prefix of the ids used by the native extensions menu.
const EXTENSION_MENU_PREFIX: &str = "pake-ext-";
/// How long a new tab may sit without starting to load before it is retried.
const STALL_RETRY: std::time::Duration = std::time::Duration::from_secs(4);
/// How many times a tab that never started is asked again. Four seconds apart,
/// so a tab that is going to load has twenty seconds of attempts behind it.
const STALL_ATTEMPTS: u32 = 5;

/// True while the active page holds an HTML fullscreen element.
static FULLSCREEN: AtomicBool = AtomicBool::new(false);

/// WebView2 can refuse a whole extension folder. Retrying every new tab with
/// the same folder only reproduces the failure, so remember it for the session;
/// the built-in ad blocker keeps working either way.
static EXTENSION_LOAD_FAILED: AtomicBool = AtomicBool::new(false);

/// The last address a new-window request asked for, with the moment it arrived.
///
/// One gesture can produce two requests — WebView2 raises its own for a
/// middle-click and the page can raise another through `window.open` — and
/// each one used to become a tab, so a single click opened two. Remembering
/// the last request makes the repeat a no-op.
static LAST_POPUP: Mutex<Option<(String, Instant)>> = Mutex::new(None);

/// Window within which a repeated request for the same address is ignored.
const POPUP_REPEAT: Duration = Duration::from_millis(700);

/// One tab, as serialized to the tab strip webview.
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
    /// Extensions that can show a settings page, for the tab strip menu.
    pub extensions: Vec<ExtensionInfo>,
}

pub struct TabEntry {
    pub id: u32,
    pub title: String,
    pub url: String,
    pub loading: bool,
    /// Set as soon as the webview reports any load event.
    started: bool,
}

/// A tab whose webview is warming its renderer before loading the real page.
struct Pending {
    target: Url,
    /// Set once the navigation to `target` has been issued, so the retry timer
    /// and the page-load handler cannot both fire it.
    dispatched: bool,
}

#[derive(Default)]
struct Tabs {
    items: Vec<TabEntry>,
    active: Option<u32>,
    next_id: u32,
    /// Tabs still showing their warm-up document instead of the real page.
    ///
    /// WebView2 attaches extension content scripts only to a renderer that
    /// already exists, so a webview created straight at the target URL runs
    /// that page without any extension. Opening a cheap page on the same
    /// origin first gives the renderer a document to be created for.
    pending: HashMap<u32, Pending>,
}

pub struct TabManager {
    state: Mutex<Tabs>,
    pub config: PakeConfig,
    pub data_dir: PathBuf,
    pub session_path: Option<PathBuf>,
    /// Folder of unpacked extensions, shared by every profile. WebView2 loads
    /// every immediate subfolder that contains a `manifest.json`.
    pub extensions_dir: Option<PathBuf>,
    /// Resolved profile number, `DEFAULT_PROFILE` when no flag was given.
    pub profile_id: u32,
    /// Cached settings-page list. WebView2 only records an extension id once it
    /// has loaded, so the cache is refreshed on demand.
    extensions: Mutex<Vec<ExtensionInfo>>,
    /// Runtime filter catalogue, injected into every tab before `adblock.js`.
    /// Loaded once at startup so replacing the JSON only needs a restart.
    pub filter_script: String,
}

impl TabManager {
    pub fn new(
        config: PakeConfig,
        data_dir: PathBuf,
        session_path: Option<PathBuf>,
        extensions_dir: Option<PathBuf>,
        profile_id: u32,
        filter_script: String,
    ) -> Self {
        Self {
            state: Mutex::new(Tabs::default()),
            config,
            data_dir,
            session_path,
            extensions_dir,
            profile_id,
            extensions: Mutex::new(Vec::new()),
            filter_script,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Tabs> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }

    /// Re-read the extension folders and their assigned ids.
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

    fn user_agent(&self) -> String {
        self.config.user_agent.get().clone()
    }
}

/// GUI-subsystem builds have no console, so `eprintln!` can vanish. Diagnostics
/// go to a file in the temp directory instead, prefixed with the time since the
/// first message so startup phases can be compared.
pub fn trace(message: &str) {
    use std::io::Write;
    use std::sync::OnceLock;
    use std::time::Instant;

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

fn tab_label(id: u32) -> String {
    format!("pake-tab-{id}")
}

fn parse_tab_id(label: &str) -> Option<u32> {
    label.strip_prefix("pake-tab-")?.parse().ok()
}

/// Open a link that points outside YouTube in the user's default browser.
///
/// The shell itself stays YouTube-only; the system browser is the right place
/// for a channel's website, a support page or any other external link.
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

/// Resize the tab strip and every content webview to match the window.
pub fn layout(app: &AppHandle) {
    let Some(window) = app.get_window(WINDOW_LABEL) else {
        return;
    };
    let (Ok(size), Ok(scale)) = (window.inner_size(), window.scale_factor()) else {
        return;
    };

    // A fullscreen video covers the whole window, tab strip included.
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

/// Show the active tab and hide the rest, then re-run layout.
fn apply_active(app: &AppHandle) {
    let manager = app.state::<TabManager>();
    let active = manager.active_id();
    // A tab that is still warming its renderer has no real page to show yet.
    let warming: Vec<u32> = match warmup_hidden() {
        true => manager.lock().pending.keys().copied().collect(),
        false => Vec::new(),
    };
    layout(app);
    for webview in app.webviews().values() {
        if webview.label() == CHROME_LABEL {
            continue;
        }
        let id = parse_tab_id(webview.label());
        if id == active && !id.is_some_and(|id| warming.contains(&id)) {
            let _ = webview.show();
            let _ = webview.set_focus();
            // A navigation issued to a hidden webview is dropped rather than
            // queued, and the tab would then sit on its warm-up document for
            // good — hidden tabs are the ones that hit this, since nothing
            // else ever asks them again. Once the tab is on screen, ask.
            if let Some(id) = id {
                resume_target(app, id, &webview);
            }
        } else {
            let _ = webview.hide();
        }
    }
}

/// Navigate a tab that was shown without ever having reached its destination.
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
    let user_agent = manager.user_agent();
    let data_dir = manager.data_dir.clone();
    let extensions = manager.extensions_dir.clone().filter(|directory| {
        allow_extensions
            && !extensions_disabled()
            && !EXTENSION_LOAD_FAILED.load(Ordering::Relaxed)
            && extensions::has_extensions(directory)
    });

    let title_app = app.clone();
    let load_app = app.clone();
    let popup_app = app.clone();
    let navigation_app = app.clone();
    let adblock_flag = if adblock_enabled() {
        "window.__YTPAKE_ADBLOCK_ENABLED__ = true;"
    } else {
        "window.__YTPAKE_ADBLOCK_ENABLED__ = false;"
    };
    let filter_script = manager.filter_script.clone();

    // WebView2 only hands extension content scripts to a renderer that already
    // exists, so the very first page of a new webview runs without them: the
    // extension appears in the menu and its options page opens, but nothing is
    // applied to the site. Loading a cheap page on the same origin first brings
    // the renderer up, and replacing it with the real address then gets the
    // scripts at document_start. Only tabs with extensions pay for the hop.
    let start = match extensions.is_some() {
        true => match warmup_url(&url) {
            Some(warmup) => {
                manager.lock().pending.insert(
                    id,
                    Pending {
                        target: url.clone(),
                        dispatched: false,
                    },
                );
                warmup
            }
            None => url,
        },
        false => {
            // The fallback build has no extensions, so it navigates directly
            // and must not leave a pending destination behind.
            manager.lock().pending.remove(&id);
            url
        }
    };

    let mut builder = WebviewBuilder::new(label, WebviewUrl::External(start))
        .user_agent(&user_agent)
        .data_directory(data_dir)
        // The shell is dark; without this the throwaway document flashes white.
        .background_color(Color(15, 15, 15, 255))
        .initialization_script(BRIDGE_JS)
        // The built-in blocker runs before YouTube's own scripts and does not
        // depend on an extension being installed. Extensions are a second,
        // optional layer on top of it.
        .initialization_script(adblock_flag)
        .initialization_script(filter_script.as_str())
        .initialization_script(ADBLOCK_JS)
        // This shell is YouTube-only. A page that tries to navigate anywhere
        // else is denied and the link is handed to the system browser instead.
        .on_navigation(move |url| {
            if youtube::allows_navigation(url) {
                return true;
            }
            open_external(&navigation_app, url);
            false
        })
        .on_new_window(move |url, _features| {
            // Sites open links with window.open; a YouTube link becomes a new
            // tab, an external one goes to the system browser. The closure
            // runs off the main thread on Windows, so spawning a tab is safe.
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
                // In the background: the page the user is on keeps the
                // focus, the way a middle click behaves in a browser. The
                // tab loads while it is out of sight.
                std::thread::spawn(move || {
                    let _ = open_tab(&app, Some(target), false);
                });
            }
            NewWindowResponse::Deny
        })
        .on_document_title_changed(move |webview, title| {
            // The throwaway warm-up document is skipped: its title is the
            // site's 404 page, and taking it would wipe a restored tab's real
            // name. The check is on the document being shown rather than on
            // the tab's pending flag, because the real page usually reports
            // its title before the load is reported as finished — gating on
            // that flag threw the title away and left the tab called
            // "Nova aba" with a video already playing in it.
            trace(&format!(
                "title {} {:?} at {:?}",
                webview.label(),
                title,
                webview.url().map(|url| url.to_string())
            ));
            if title.trim().is_empty() {
                return;
            }
            if let Ok(url) = webview.url() {
                if url.path() == WARMUP_PATH {
                    return;
                }
            }
            if let Some(id) = parse_tab_id(webview.label()) {
                update_title(&title_app, id, &title);
            }
        })
        .on_page_load(move |webview, payload| {
            if let Some(id) = parse_tab_id(webview.label()) {
                let finished = matches!(payload.event(), tauri::webview::PageLoadEvent::Finished);
                update_loading(&load_app, id, payload.url(), finished);
            }
        });

    // Only tabs that load extensions have to queue: without them there is
    // nothing to register, and waiting would just slow the app down.
    let queued = extensions.is_some();

    if let Some(directory) = extensions {
        // WebView2 needs the environment flag before it will accept a profile
        // extension; Tauri forwards both to wry.
        builder = builder
            .browser_extensions_enabled(true)
            .extensions_path(directory);
    }

    #[cfg(target_os = "windows")]
    {
        // wry replaces its defaults when this is set, so keep them and add
        // autoplay: YouTube should start a video on click, not need a second one.
        builder = builder.additional_browser_args(
            "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection \
             --autoplay-policy=no-user-gesture-required",
        );
    }

    let window = app
        .get_window(WINDOW_LABEL)
        .ok_or_else(|| tauri::Error::WindowNotFound)?;
    // Only tabs that load extensions have to queue: without them there is
    // nothing to register, and waiting would just slow the app down.
    if queued {
        wait_for_creation_slot();
    }

    let webview = window.add_child(
        builder,
        PhysicalPosition::new(0, strip as i32),
        PhysicalSize::new(width, height),
    )?;
    trace(&format!("tab {id}: webview added"));

    // A tab still warming its renderer shows a throwaway document; keep it off
    // screen until the real page starts loading.
    if warmup_hidden() && manager.lock().pending.contains_key(&id) {
        let _ = webview.hide();
    }
    Ok(webview)
}

/// The throwaway page a tab loads before its real address.
///
/// Three things it has to be, and the third one is easy to miss:
///
/// - On the target's own origin, so the extensions treat it as the real site.
/// - HTML. `robots.txt` is the cheapest thing a site serves and it does bring
///   the renderer up, but it arrives as `text/plain` and content scripts do not
///   run there at all, so it warmed the renderer and never the extensions.
/// - A different address from the destination. A warm-up document at the same
///   address is not a second load — the page keeps whatever the extension
///   worker handed out while it was still starting, which is why the home page
///   kept its ads until a reload.
///
/// A page that does not exist satisfies all three for the price of a 404: on
/// YouTube it is a 758-byte `text/html` document answered in about 190 ms,
/// against a couple of seconds for the home page it used to load.
fn warmup_url(target: &Url) -> Option<Url> {
    if !matches!(target.scheme(), "http" | "https") {
        return None;
    }
    let host = target.host_str()?;

    let mut warmup = Url::parse(&format!("{}://{host}", target.scheme())).ok()?;
    if let Some(port) = target.port() {
        warmup.set_port(Some(port)).ok()?;
    }
    warmup.set_path(WARMUP_PATH);
    warmup.set_query(None);
    Some(warmup)
}

/// How long a tab keeps its warm-up document before loading the real page.
///
/// The value is what the extension host needs to register the renderer: a
/// document created before the host is ready never receives the content
/// scripts, which showed up as one tab in three coming up without the
/// extension. The throwaway page is HTML, so the host is already working on it
/// by the time the document has finished loading, and the wait on top of that
/// only has to cover the reply.
///
/// Measured on this machine: 400 ms held over two runs of three tabs, and zero
/// is where it breaks. This sits above that measurement rather than at the
/// edge. The value is the same for every tab — the first tab's extra second
/// only ever paid for the service worker, which the HTML warm-up page now wakes
/// on its own.
/// `PAKE_WARMUP_DELAY_MS` overrides it so the threshold can be measured on a
/// machine without rebuilding.
fn warmup_delay() -> Duration {
    static DELAY: OnceLock<Option<Duration>> = OnceLock::new();

    let override_ms = *DELAY.get_or_init(|| {
        std::env::var("PAKE_WARMUP_DELAY_MS")
            .ok()
            .and_then(|raw| raw.trim().parse::<u64>().ok())
            .map(Duration::from_millis)
    });
    override_ms.unwrap_or(Duration::from_millis(500))
}

/// Minimum gap between two tab webviews while extensions are loaded.
///
/// WebView2 registers an extension's content scripts with a webview once, and
/// it cannot do that for several webviews at the same time: four tabs created
/// together all end up without the scripts, while the same tabs created a few
/// seconds apart all keep them. Four seconds is the smallest gap that held up
/// in measurement.
const TAB_SPACING: Duration = Duration::from_millis(4_000);

/// When the previous tab webview was created.
static LAST_CREATE: Mutex<Option<Instant>> = Mutex::new(None);

/// Wait until this tab may be created without racing the previous one.
///
/// The lock is held across the wait, which is what serializes the callers.
fn wait_for_creation_slot() {
    let mut last = LAST_CREATE
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if let Some(at) = *last {
        let elapsed = at.elapsed();
        if elapsed < TAB_SPACING {
            let wait = TAB_SPACING - elapsed;
            trace(&format!("slot: waiting {}ms", wait.as_millis()));
            std::thread::sleep(wait);
        }
    }
    *last = Some(Instant::now());
}

/// Whether a tab hides itself while it warms its renderer.
///
/// Overridable through `PAKE_WARMUP_HIDE=0` so the effect of a hidden webview
/// on the extension host can be measured without rebuilding.
fn warmup_hidden() -> bool {
    static HIDDEN: OnceLock<bool> = OnceLock::new();

    *HIDDEN.get_or_init(|| {
        std::env::var("PAKE_WARMUP_HIDE")
            .map(|raw| raw.trim() != "0")
            .unwrap_or(true)
    })
}

/// True when the same address was just turned into a tab.
///
/// Recorded before the tab is created, so the two requests of a single
/// gesture cannot both pass. The lock is poisoned only if a panic happens
/// while it is held, and the value is still usable then.
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

/// Show the settings of an extension, embedded in the active tab.
///
/// WebView2 refuses `chrome-extension://` as a top-level document, so the page
/// is opened in an iframe overlay instead. `extensions::settings_pages` makes
/// sure the page is embeddable first.
fn open_extension_settings(app: &AppHandle, url: &str) -> Result<(), String> {
    if !url.starts_with("chrome-extension://") {
        return Err("endereço de extensão inválido".into());
    }
    let webview = active_webview(app).ok_or_else(|| "nenhuma aba ativa".to_string())?;
    let payload = serde_json::to_string(url).map_err(|error| error.to_string())?;
    webview
        .eval(&format!(
            "window.__pakeOpenSettings && window.__pakeOpenSettings({payload})"
        ))
        .map_err(|error| error.to_string())
}

/// Open the extensions menu at the cursor.
///
/// A single extension is opened straight away; two or more get a native popup
/// menu, which is the only kind of menu that can extend past the 40px tab strip.
#[tauri::command]
pub async fn tabs_extension_menu(app: AppHandle) -> Result<(), String> {
    let manager = app.state::<TabManager>();
    let found = manager.refresh_extensions();
    if found.is_empty() {
        return Err("nenhuma extensão com página de configuração".into());
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
        .ok_or_else(|| "janela indisponível".to_string())?;
    window.popup_menu(&menu).map_err(|error| error.to_string())
}

/// Add a tab to the strip and return its id and address, without a webview.
///
/// Keeping the two steps apart lets a restored session put every tab on screen
/// at once while the webviews are built one at a time behind them.
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

/// Build a reserved tab's webview and start loading its page.
fn create_webview(app: &AppHandle, id: u32, target: Url) -> Result<(), String> {
    let manager = app.state::<TabManager>();
    let (width, height, strip) = window_metrics(app);

    // A single extension WebView2 refuses to load aborts webview creation, so
    // the tab is retried without extensions and they stay off for the session.
    let mut built = build_webview(app, id, target.clone(), strip, width, height, true);
    if built.is_err() && !extensions_disabled() {
        trace("extension loading failed; opening tabs without extensions for this session");
        EXTENSION_LOAD_FAILED.store(true, Ordering::Relaxed);
        built = build_webview(app, id, target.clone(), strip, width, height, false);
    }
    if let Err(error) = built {
        trace(&format!("failed to open a tab at {target}: {error}"));
        // Roll back so the tab strip never shows a tab without a webview.
        let mut state = manager.lock();
        state.items.retain(|tab| tab.id != id);
        state.pending.remove(&id);
        if state.active == Some(id) {
            state.active = state.items.first().map(|tab| tab.id);
        }
        return Err(error.to_string());
    }

    // Safety net: a tab stuck on its warm-up document would stay invisible and
    // never reach the site, so force the destination after a few seconds. The
    // normal path is not used here on purpose: the point of the timeout is
    // that it did not fire, so its "already sent" flag must not block this.
    let fallback = app.clone();
    let destination = target.clone();
    std::thread::spawn(move || {
        // Long enough for the normal path to finish its own warm-up. The
        // warm-up document has to finish loading before the normal path even
        // starts counting, so the margin covers that load as well as the delay
        // itself.
        std::thread::sleep(warmup_delay() + Duration::from_millis(6_000));
        let manager = fallback.state::<TabManager>();
        if !manager.lock().pending.contains_key(&id) {
            return;
        }
        trace(&format!("tab {id}: warm-up timed out"));

        if let Some(webview) = fallback.get_webview(&tab_label(id)) {
            if let Err(error) = webview.navigate(destination) {
                trace(&format!("tab {id}: timed-out navigation failed: {error}"));
            }
        }

        // Give the page a moment, then stop treating the tab as warming so a
        // failure here cannot leave it invisible forever.
        let handle = fallback.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(2_000));
            let manager = handle.state::<TabManager>();
            manager.lock().pending.remove(&id);
            apply_active(&handle);
        });
    });
    apply_active(app);
    emit_state(app);
    save_session(app);
    watch_for_stall(app.clone(), id, target);
    Ok(())
}

/// Open a tab, optionally pointing at `url`, and return its id.
pub fn open_tab(app: &AppHandle, url: Option<String>, activate: bool) -> Result<u32, String> {
    let (id, target) = reserve_tab(app, url.as_deref(), activate);
    create_webview(app, id, target)?;
    Ok(id)
}

/// Re-navigate a tab that never started loading.
///
/// WebView2 drops a navigation now and then when extensions are enabled, and a
/// tab that never starts would sit on its throwaway document for good. One
/// retry used to be enough; the retry itself can be dropped too, so the tab is
/// asked a few times. Every attempt is conditional on the tab still not having
/// started, so a page that loads stops the loop on the next tick.
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

/// Put a tab title in the window caption.
///
/// Used both when the active tab's own title changes and when another tab
/// becomes active: switching tabs used to leave the previous tab's name in the
/// caption, because only a title change ever wrote it.
fn set_window_title(app: &AppHandle, display: &str) {
    let profile_id = app.state::<TabManager>().profile_id;
    let Some(window) = app.get_window(WINDOW_LABEL) else {
        return;
    };
    // The home page is already called "YouTube"; suffixing it would read
    // "YouTube - YouTube".
    let mut title = if display.eq_ignore_ascii_case("youtube") {
        display.to_string()
    } else {
        format!("{display} - YouTube")
    };
    // Keep the profile visible in the taskbar while several run at once.
    if profile_id != DEFAULT_PROFILE {
        title.push_str(&format!(" (perfil {profile_id})"));
    }
    let _ = window.set_title(&title);
}

/// Re-apply the active tab's title to the window caption.
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
    // While a tab warms its renderer the document on screen is a throwaway, so
    // the tab keeps its real address, title and loading state until the real
    // page starts loading.
    let manager = app.state::<TabManager>();
    let mut reveal = false;
    let dispatch = {
        let mut state = manager.lock();
        let is_target = state
            .pending
            .get(&id)
            .is_some_and(|entry| entry.target == *url);

        if is_target {
            // The real page is finally loading: back to normal handling, and
            // the tab can come on screen.
            state.pending.remove(&id);
            reveal = true;
            false
        } else if state.pending.contains_key(&id) {
            // The warm-up document finished. `send_to_target` owns the
            // `dispatched` flag, so the retry timer and this event cannot
            // both navigate: whichever arrives second is turned away there.
            if finished {
                true
            } else {
                return;
            }
        } else {
            false
        }
    };
    if dispatch {
        send_to_target(app, id);
        return;
    }
    if reveal {
        apply_active(app);
    }

    let manager = app.state::<TabManager>();
    {
        let mut state = manager.lock();
        let Some(tab) = state.items.iter_mut().find(|tab| tab.id == id) else {
            return;
        };
        tab.loading = !finished;
        tab.started = true;
        tab.url = url.to_string();
    }
    emit_state(app);
    if finished {
        save_session(app);
        // WebView2 only records an extension id once it has loaded, so this is
        // the earliest point the settings menu can be filled in.
        let manager = app.state::<TabManager>();
        if manager.snapshot().extensions.is_empty() && !manager.refresh_extensions().is_empty() {
            emit_state(app);
        }
        watch_for_extensions(app);
    }
}

/// Replace a tab's warm-up document with the real destination.
///
/// The pause is deliberate: the renderer exists as soon as the warm-up
/// document is committed, but the extension host only learns about it a
/// moment later. Navigating before that reply arrives leaves the page without
/// content scripts, which is the whole reason for the hop.
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

    let handle = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(warmup_delay());
        trace(&format!("tab {id}: warm-up over, sending the real address"));

        let Some(webview) = handle.get_webview(&tab_label(id)) else {
            trace(&format!("tab {id}: closed before its first navigation"));
            return;
        };

        // A real navigation, not `location.replace`. WebView2 only attaches
        // the extensions' content scripts to a document the browser asked
        // for; a navigation started from the warm-up document's own script
        // gets none, which is the failure this hop exists to avoid. The cost
        // is one extra back-history entry pointing at the warm-up page.
        if let Err(error) = webview.navigate(target) {
            trace(&format!("tab {id}: first navigation failed: {error}"));
        }
        // The tab stays hidden until the real page starts loading, so the
        // warm-up document is never on screen.
    });
}

/// Keep looking for extensions for a few seconds after a page load.
///
/// On a fresh profile the ids are written to the browser profile a moment after
/// the first page finishes, so the attempt above can come back empty and leave
/// the settings button hidden until the next navigation. Retrying costs nothing
/// once the list is known.
fn watch_for_extensions(app: &AppHandle) {
    static PENDING: AtomicBool = AtomicBool::new(false);

    let manager = app.state::<TabManager>();
    if !manager.snapshot().extensions.is_empty() {
        return;
    }
    if PENDING.swap(true, Ordering::AcqRel) {
        return;
    }

    let handle = app.clone();
    std::thread::spawn(move || {
        for delay in [700u64, 1_500, 3_000, 6_000, 12_000] {
            std::thread::sleep(std::time::Duration::from_millis(delay));
            let manager = handle.state::<TabManager>();
            if !manager.refresh_extensions().is_empty() {
                emit_state(&handle);
                break;
            }
        }
        PENDING.store(false, Ordering::Release);
    });
}

/// Build a tab's webview if it does not exist yet.
///
/// Tabs restored from the previous session are only reserved: the app opens
/// with a single page loading instead of one per tab, and the rest are built
/// the first time they are shown. Building them all at once is what stops the
/// extension host from registering any of them.
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
            return Err("aba inexistente".into());
        }
        if state.active == Some(id) {
            // Already selected, but the webview may still be waiting to be
            // built the first time.
            drop(state);
            ensure_webview(app, id);
            apply_active(app);
            return Ok(());
        }
        let previous = state.active;
        state.active = Some(id);
        previous
    };

    // Switching away from a fullscreen video would otherwise leave the tab
    // strip hidden with no way back to it.
    if FULLSCREEN.swap(false, Ordering::Relaxed) {
        if let Some(previous) = previous.and_then(|id| app.get_webview(&tab_label(id))) {
            let _ = previous.eval("document.fullscreenElement && document.exitFullscreen()");
        }
    }

    // A restored tab has no webview until it is first shown.
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
            .ok_or_else(|| "aba inexistente".to_string())?;
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
        // The tab that takes over may still be waiting to be built.
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
            return Err("nenhuma aba".into());
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
            return Err("nenhuma aba".into());
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

// ---------------------------------------------------------------------------
// Session persistence
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Default)]
struct Session {
    tabs: Vec<String>,
    /// Tab titles, so a restored tab shows its name before it is loaded.
    /// Missing in sessions written by older builds, hence the default.
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

    // Take the snapshot under the lock, then resolve each tab's live URL
    // outside it: `url()` round-trips to the main thread and must never run
    // while another thread could be waiting on this mutex.
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

    // A YouTube route change is a pushState, not a navigation, so the URL
    // stored at load time goes stale; the webview's own URL is authoritative.
    let mut tabs: Vec<String> = Vec::with_capacity(entries.len());
    let mut titles: Vec<String> = Vec::with_capacity(entries.len());
    for (id, fallback, title) in entries {
        let url = app
            .get_webview(&tab_label(id))
            .and_then(|webview| webview.url().ok())
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

// ---------------------------------------------------------------------------
// Commands (called from the tab strip and from page shortcuts)
// ---------------------------------------------------------------------------

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
        .ok_or_else(|| "nenhuma aba ativa".to_string())?;
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
    let webview = active_webview(&app).ok_or_else(|| "nenhuma aba ativa".to_string())?;
    webview.navigate(target).map_err(|error| error.to_string())
}

/// Open an external link in the system browser, never inside a tab.
///
/// The page-side bridge calls this for links outside the YouTube allowlist;
/// the shell itself stays a single-site application.
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
    let webview = active_webview(&app).ok_or_else(|| "nenhuma aba ativa".to_string())?;
    webview.reload().map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn tabs_back(app: AppHandle) -> Result<(), String> {
    let webview = active_webview(&app).ok_or_else(|| "nenhuma aba ativa".to_string())?;
    webview
        .eval("history.back()")
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn tabs_forward(app: AppHandle) -> Result<(), String> {
    let webview = active_webview(&app).ok_or_else(|| "nenhuma aba ativa".to_string())?;
    webview
        .eval("history.forward()")
        .map_err(|error| error.to_string())
}

/// Grow the active tab over the tab strip while a video is fullscreen.
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

// ---------------------------------------------------------------------------
// App bootstrap
// ---------------------------------------------------------------------------

fn create_shell(app: &AppHandle) -> tauri::Result<()> {
    trace("shell: start");
    let manager = app.state::<TabManager>();
    let window_config = manager.config.windows.first().cloned();

    let width = window_config.as_ref().map(|c| c.width).unwrap_or(1440.0);
    let height = window_config.as_ref().map(|c| c.height).unwrap_or(900.0);
    let title = window_config
        .as_ref()
        .and_then(|c| c.title.clone())
        .unwrap_or_else(|| "YouTube".to_string());
    // Make it obvious which profile a window belongs to when several run at once.
    let title = if manager.profile_id == DEFAULT_PROFILE {
        title
    } else {
        format!("{title} (perfil {})", manager.profile_id)
    };

    let window = tauri::window::WindowBuilder::new(app, WINDOW_LABEL)
        .title(title)
        .inner_size(width, height)
        .min_inner_size(560.0, 400.0)
        .resizable(true)
        .center()
        .visible(false)
        // Painted while the first webview is still being built, so the wait
        // reads as the app starting rather than as a white flash.
        .background_color(tauri::window::Color(15, 15, 15, 255))
        .build()?;

    if let Some(icon) = app.default_window_icon().cloned() {
        let _ = window.set_icon(icon);
    }

    let size = window.inner_size()?;

    // Show the frame before building any webview. `show` is handled by the
    // event loop, so the webviews have to be built after the setup hook returns;
    // otherwise the first WebView2 (which loads every unpacked extension) keeps
    // the window invisible for tens of seconds and the launch looks broken.
    let _ = window.show();
    let _ = window.set_focus();
    trace("shell: shown");

    let shell = app.clone();
    tauri::async_runtime::spawn(async move {
        create_strip(&shell, size);
        restore_tabs(&shell);
    });

    Ok(())
}

/// Build the tab strip, the first webview of the window.
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

/// Reopen the tabs saved by the previous run, or the home page on a first run.
///
/// The session only reserves the tabs; the page the user was on is built right
/// away and the others follow the first time they are shown. This runs off the
/// main thread so a slow first page cannot hold up the window.
fn restore_tabs(app: &AppHandle) {
    let handle = app.clone();
    std::thread::spawn(move || restore_tabs_now(&handle));
}

fn restore_tabs_now(app: &AppHandle) {
    let manager = app.state::<TabManager>();
    let session_path = manager.session_path.clone();

    let Some(session) = load_session(&session_path).filter(|session| !session.tabs.is_empty())
    else {
        // open_tab activates this one itself; nothing else to select.
        let _ = open_tab(app, None, true);
        layout(app);
        emit_state(app);
        trace("tabs: restored");
        return;
    };

    let active = session.active.min(session.tabs.len().saturating_sub(1));

    // Reserve every tab first so the strip is complete straight away, then
    // build only the one the user was on. The rest are built when they are
    // first shown: building them all here is what makes the extension host
    // skip most of them, and it also keeps startup to a single page load.
    let mut reserved: Vec<u32> = Vec::with_capacity(session.tabs.len());
    for url in &session.tabs {
        reserved.push(reserve_tab(app, Some(url), false).0);
    }
    {
        let mut state = manager.lock();
        for (index, tab) in state.items.iter_mut().enumerate() {
            tab.loading = false;
            // A tab that is not loaded yet has no title of its own, so the one
            // saved with the session is what the strip shows until then.
            if let Some(title) = session.titles.get(index) {
                if !title.trim().is_empty() {
                    tab.title = title.clone();
                }
            }
        }
    }
    emit_state(app);

    if let Some(id) = reserved.get(active) {
        let _ = activate_tab(app, *id);
    }

    layout(app);
    emit_state(app);
    trace("tabs: restored");
}

pub fn run_tabbed(pake_config: PakeConfig, tauri_config: Config) {
    // Each profile is its own app as far as Windows is concerned: a distinct
    // AppUserModelID groups its taskbar button separately, so two profiles can
    // sit side by side instead of stacking under one icon.
    let profile_id = requested_profile();
    let identifier = match profile_id {
        Some(id) => format!("{}.profile{id}", tauri_config.identifier),
        None => tauri_config.identifier.clone(),
    };

    #[cfg(windows)]
    crate::util::set_app_user_model_id(&identifier);

    let context = tauri::generate_context!();

    let mut builder = tauri::Builder::default();

    if profile_id.is_none() {
        // Named profiles are meant to run concurrently, so the single-instance
        // guard only covers the default one.
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_window(WINDOW_LABEL) {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
        }));
    }

    // Window geometry is remembered per profile, otherwise two profiles would
    // open on top of each other at the same coordinates.
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
            // Native popup menu built on demand by `tabs_extension_menu`.
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
            // Grant IPC to the configured site origin. Without this the page
            // bridge cannot reach the tab commands, so shortcuts and
            // middle-click tabs would silently do nothing.
            if let Some(capability) = pake_config.remote_capability()? {
                app.add_capability(capability)?;
            }

            let handle = app.app_handle().clone();
            let app_root = handle.path().app_data_dir().ok();
            let profile = resolve_profile(app_root.as_deref());

            // Settings pages have to be embeddable before the browser reads
            // the manifests, so this runs ahead of `create_shell`.
            extensions::prepare(&profile.extensions);
            extensions::log_discovered(&profile.extensions);
            trace(&format!(
                "setup: extensions folder {}",
                profile.extensions.display()
            ));

            let data_dir = profile.webview_dir();
            let session_path = Some(profile.session_path());
            // One folder beside the executable serves every profile.
            let extensions_dir = Some(profile.extensions.clone());
            // Filters are read once here, not per tab: a replaced JSON takes
            // effect on the next launch for every tab in the window.
            let filter_script = filters::initialization_script(&profile.base);

            app.manage(TabManager::new(
                pake_config.clone(),
                data_dir,
                session_path,
                extensions_dir,
                profile.id,
                filter_script,
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
