//! Runtime filter catalogue for the built-in ad blocker.
//!
//! The lists are data, not code. `filters/youtube.json` is the source of truth
//! and is compiled into the binary as a baseline, but at startup the app looks
//! for a newer copy on disk first. That means YouTube changing its ad markup
//! can be answered by replacing a JSON file — no rebuild, no reinstall.
//!
//! Lookup order (first valid catalogue wins):
//!   1. `--filters <path>` or `YTPAKE_FILTERS_FILE`
//!   2. `<app data>/filters/youtube.json` — user drop-in, survives upgrades
//!   3. `<exe dir>/filters/youtube.json` — copy shipped with the installer
//!   4. the JSON compiled into the binary
//!
//! An invalid or incomplete file is skipped with a trace line rather than
//! leaving the app without a blocker.

use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

use crate::app::tabs::trace;

/// Always available: the repository copy, embedded at compile time.
const BUNDLED_JSON: &str = include_str!("../../../filters/youtube.json");

const FILE_NAME: &str = "youtube.json";

/// The initialization script injected ahead of `adblock.js`.
pub fn initialization_script(base: &Path) -> String {
    let (catalogue, source) = load(base);
    let version = catalogue
        .get("version")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    trace(&format!("adblock filters: {source} (version {version})"));

    let loaded_from = serde_json::to_string(&source).unwrap_or_else(|_| "\"unknown\"".to_string());
    format!(
        "window.__YTPAKE_ADBLOCK_FILTERS__ = {catalogue}; \
         window.__YTPAKE_ADBLOCK_FILTERS__.loadedFrom = {loaded_from};"
    )
}

/// Read and validate the first available catalogue.
fn load(base: &Path) -> (Value, String) {
    for (path, label) in candidates(base) {
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(catalogue) = serde_json::from_str::<Value>(&text) else {
            trace(&format!(
                "adblock filters: invalid JSON in {}, skipping",
                path.display()
            ));
            continue;
        };
        if !is_valid(&catalogue) {
            trace(&format!(
                "adblock filters: incomplete catalogue in {}, skipping",
                path.display()
            ));
            continue;
        }
        return (catalogue, format!("{label} {}", path.display()));
    }

    let catalogue =
        serde_json::from_str(BUNDLED_JSON).expect("the bundled filter catalogue is valid JSON");
    (catalogue, "compiled-in baseline".to_string())
}

/// Check the shape `adblock.js` expects before injecting anything.
fn is_valid(catalogue: &Value) -> bool {
    let required = ["adKeys", "breakKeys", "urlPatterns", "selectors"];
    required.iter().all(|&key| {
        catalogue
            .get(key)
            .and_then(Value::as_array)
            .is_some_and(|items| !items.is_empty() && items.iter().all(Value::is_string))
    }) && catalogue
        .get("containerSelectors")
        .and_then(Value::as_array)
        .is_some_and(|items| items.iter().all(Value::is_string))
        && catalogue.get("version").and_then(Value::as_str).is_some()
}

fn candidates(base: &Path) -> Vec<(PathBuf, &'static str)> {
    let mut found = Vec::new();

    if let Some(path) = override_path() {
        found.push((path, "explicit override"));
    }
    found.push((base.join("filters").join(FILE_NAME), "app data"));
    if let Some(exe_dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
    {
        found.push((exe_dir.join("filters").join(FILE_NAME), "bundled"));
    }
    found
}

fn override_path() -> Option<PathBuf> {
    if let Ok(raw) = std::env::var("YTPAKE_FILTERS_FILE") {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed));
        }
    }
    filters_file_from_args(std::env::args().skip(1))
}

/// Pure argument parser, unit-testable without the process command line.
fn filters_file_from_args<I>(args: I) -> Option<PathBuf>
where
    I: IntoIterator<Item = String>,
{
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        if arg == "--filters" {
            if let Some(value) = args.next() {
                let value = value.trim();
                if !value.is_empty() {
                    return Some(PathBuf::from(value));
                }
            }
        } else if let Some(value) = arg.strip_prefix("--filters=") {
            let value = value.trim();
            if !value.is_empty() {
                return Some(PathBuf::from(value));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn the_bundled_catalogue_is_complete() {
        let catalogue: Value = serde_json::from_str(BUNDLED_JSON).expect("valid JSON");
        assert!(is_valid(&catalogue));
        // Deliberately not pinned to a literal: updating the filter version is
        // a routine operation and must not break a unit test.
        assert!(catalogue
            .get("version")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty()));
    }

    #[test]
    fn an_incomplete_catalogue_is_rejected() {
        let incomplete = serde_json::json!({ "version": "1", "selectors": ["a"] });
        assert!(!is_valid(&incomplete));
        let wrong_type = serde_json::json!({
            "version": "1",
            "adKeys": [1],
            "breakKeys": ["adPlacements"],
            "urlPatterns": ["doubleclick"],
            "selectors": ["a"],
            "containerSelectors": []
        });
        assert!(!is_valid(&wrong_type));
    }

    #[test]
    fn the_override_flag_accepts_both_forms() {
        assert_eq!(
            filters_file_from_args(args(&["--profile2", "--filters", "C:/f.json"])),
            Some(PathBuf::from("C:/f.json"))
        );
        assert_eq!(
            filters_file_from_args(args(&["--filters=C:/f.json"])),
            Some(PathBuf::from("C:/f.json"))
        );
        assert_eq!(filters_file_from_args(args(&["--filters"])), None);
        assert_eq!(filters_file_from_args(args(&["--filters="])), None);
        assert_eq!(filters_file_from_args(args(&["--no-adblock"])), None);
    }
}
