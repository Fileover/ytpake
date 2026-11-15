fn main() {
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "tabs_new",
            "tabs_close",
            "tabs_close_active",
            "tabs_activate",
            "tabs_select",
            "tabs_cycle",
            "tabs_navigate",
            "tabs_reload",
            "tabs_back",
            "tabs_forward",
            "tabs_state",
            "tabs_fullscreen",
            "tabs_extension_menu",
            "tabs_open_external",
        ]),
    ))
    .expect("Failed to build Tauri permissions")
}
