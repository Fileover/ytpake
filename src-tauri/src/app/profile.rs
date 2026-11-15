//! Per-profile storage layout.

use std::fs;
use std::path::{Path, PathBuf};

/// `--profile1` .. `--profile99`.
pub const MAX_PROFILE: u32 = 99;

/// Used when the command line names no profile.
pub const DEFAULT_PROFILE: u32 = 1;

/// Presence of any of these next to the executable switches to portable mode.
const PORTABLE_MARKERS: [&str; 2] = ["portable", "portable.txt"];

/// Folders that used to live directly in `<base>` before profiles were mandatory.
const LEGACY_ENTRIES: [&str; 2] = ["webview", "session.json"];

#[derive(Debug, Clone)]
pub struct Profile {
    pub id: u32,
    pub dir: PathBuf,
    pub base: PathBuf,
    pub extensions: PathBuf,
}

impl Profile {
    pub fn webview_dir(&self) -> PathBuf {
        self.dir.join("webview")
    }

    pub fn session_path(&self) -> PathBuf {
        self.dir.join("session.json")
    }

    fn adopt_legacy_extensions(&self) {
        let legacy = self.dir.join("extensions");
        if legacy == self.extensions || !legacy.is_dir() {
            return;
        }

        if let Ok(entries) = fs::read_dir(&legacy) {
            for entry in entries.flatten() {
                let source = entry.path();
                if !source.is_dir() || !source.join("manifest.json").is_file() {
                    continue;
                }
                let destination = self.extensions.join(entry.file_name());
                if !destination.exists() {
                    let _ = copy_tree(&source, &destination);
                }
            }
        }

        let _ = fs::remove_dir_all(&legacy);
    }
}

pub fn resolve(app_data: Option<&Path>) -> Profile {
    let exe_dir = exe_dir();
    let portable = is_portable_in(exe_dir.as_deref());

    let base = if portable {
        exe_dir.clone().unwrap_or_else(|| PathBuf::from("."))
    } else {
        app_data
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."))
    };

    let id = requested().unwrap_or(DEFAULT_PROFILE);
    let dir = base.join(format!("profile{id}"));

    let extensions =
        requested_extensions_dir().unwrap_or_else(|| resolve_extensions_dir(exe_dir.as_deref(), &base));

    let profile = Profile {
        id,
        dir,
        base,
        extensions,
    };

    if id == DEFAULT_PROFILE && !profile.webview_dir().exists() {
        migrate_legacy(&profile.base, &profile.dir);
    }

    let _ = fs::create_dir_all(&profile.dir);
    let _ = fs::create_dir_all(&profile.extensions);
    profile.adopt_legacy_extensions();
    profile
}

fn requested_extensions_dir() -> Option<PathBuf> {
    if let Ok(raw) = std::env::var("YTPAKE_EXTENSIONS_DIR") {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed));
        }
    }
    extensions_dir_from_args(std::env::args().skip(1))
}

fn extensions_dir_from_args<I>(args: I) -> Option<PathBuf>
where
    I: IntoIterator<Item = String>,
{
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        if arg == "--extensions-dir" {
            if let Some(value) = args.next() {
                let value = value.trim();
                if !value.is_empty() {
                    return Some(PathBuf::from(value));
                }
            }
        } else if let Some(value) = arg.strip_prefix("--extensions-dir=") {
            let value = value.trim();
            if !value.is_empty() {
                return Some(PathBuf::from(value));
            }
        }
    }
    None
}

/// Resolve `.\extensions` beside the executable first, falling back to working
/// directory `.\extensions` (or `..\extensions` in workspace builds) when
/// running from a build output folder, and finally `<base>\extensions`.
fn resolve_extensions_dir(exe_dir: Option<&Path>, base: &Path) -> PathBuf {
    let beside_exe = exe_dir.map(|dir| dir.join("extensions"));
    if let Some(ref candidate) = beside_exe {
        if has_manifest_subdir(candidate) {
            return candidate.clone();
        }
    }

    if let Ok(cwd) = std::env::current_dir() {
        let cwd_ext = cwd.join("extensions");
        if has_manifest_subdir(&cwd_ext) {
            return cwd_ext;
        }
        if let Some(parent) = cwd.parent() {
            let parent_ext = parent.join("extensions");
            if has_manifest_subdir(&parent_ext) {
                return parent_ext;
            }
        }
    }

    if let Some(candidate) = beside_exe {
        if candidate.is_dir() || fs::create_dir_all(&candidate).is_ok() {
            return candidate;
        }
    }
    base.join("extensions")
}

fn has_manifest_subdir(folder: &Path) -> bool {
    fs::read_dir(folder)
        .map(|entries| {
            entries.flatten().any(|entry| {
                entry.file_type().is_ok_and(|ft| ft.is_dir())
                    && entry.path().join("manifest.json").is_file()
            })
        })
        .unwrap_or(false)
}

fn migrate_legacy(base: &Path, target: &Path) {
    if !LEGACY_ENTRIES.iter().any(|name| base.join(name).exists()) {
        return;
    }
    if fs::create_dir_all(target).is_err() {
        return;
    }

    for name in LEGACY_ENTRIES {
        let from = base.join(name);
        if !from.exists() {
            continue;
        }
        let to = target.join(name);
        if to.exists() {
            continue;
        }
        if fs::rename(&from, &to).is_err() {
            let _ = copy_tree(&from, &to);
            let _ = fs::remove_dir_all(&from);
        }
    }
}

fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    if from.is_file() {
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(from, to)?;
        return Ok(());
    }
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        copy_tree(&entry.path(), &to.join(entry.file_name()))?;
    }
    Ok(())
}

fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf))
}

fn is_portable_in(exe_dir: Option<&Path>) -> bool {
    exe_dir
        .map(|dir| {
            PORTABLE_MARKERS
                .iter()
                .any(|marker| dir.join(marker).exists())
        })
        .unwrap_or(false)
}

pub fn extensions_disabled() -> bool {
    std::env::args()
        .skip(1)
        .any(|arg| arg == "--no-extensions" || arg == "--no-ext")
}

pub fn requested() -> Option<u32> {
    let mut args = std::env::args().skip(1);

    while let Some(arg) = args.next() {
        let trimmed = arg.trim_start_matches('-');
        let Some(rest) = trimmed.strip_prefix("profile") else {
            continue;
        };
        let rest = rest.trim_start_matches(['=', ' ', ':']);

        if rest.is_empty() {
            if let Some(id) = args.next().as_deref().and_then(parse_id) {
                return Some(id);
            }
        } else if let Some(id) = parse_id(rest) {
            return Some(id);
        }
    }

    None
}

fn parse_id(raw: &str) -> Option<u32> {
    let id: u32 = raw.trim().parse().ok()?;
    (1..=MAX_PROFILE).contains(&id).then_some(id)
}

#[cfg(test)]
mod tests {
    use super::{extensions_dir_from_args, parse_id};

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn accepts_only_the_documented_range() {
        assert_eq!(parse_id("1"), Some(1));
        assert_eq!(parse_id("99"), Some(99));
        assert_eq!(parse_id("0"), None);
        assert_eq!(parse_id("100"), None);
        assert_eq!(parse_id("abc"), None);
    }

    #[test]
    fn reads_an_explicit_extensions_folder_in_both_flag_forms() {
        assert_eq!(
            extensions_dir_from_args(args(&["--profile2", "--extensions-dir", "C:/ext"])),
            Some(std::path::PathBuf::from("C:/ext"))
        );
        assert_eq!(
            extensions_dir_from_args(args(&["--extensions-dir=C:/ext"])),
            Some(std::path::PathBuf::from("C:/ext"))
        );
    }

    #[test]
    fn ignores_an_empty_or_missing_extensions_folder() {
        assert_eq!(extensions_dir_from_args(args(&["--profile2"])), None);
        assert_eq!(extensions_dir_from_args(args(&["--extensions-dir"])), None);
        assert_eq!(extensions_dir_from_args(args(&["--extensions-dir="])), None);
        assert_eq!(
            extensions_dir_from_args(args(&["--extensions-dir=", "C:/fallback"])),
            None
        );
    }
}
