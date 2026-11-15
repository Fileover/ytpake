mod app;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    app::tabs::run_tabbed();
}
