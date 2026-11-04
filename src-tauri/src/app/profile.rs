//! Per-profile storage layout.
//!
//! A profile owns everything that should stay isolated between sessions: the
//! WebView2 user data folder (cookies, cache, site data) and the tab session.
//!
//! ```text
//! <base>/profile1          default profile
//! <base>/profile2          --profile2
//! ```
//!
//! Every profile is a numbered folder, including the default one, so the layout
//! is uniform: `--profile1` and no flag mean the same thing.
//!
//! `<base>` is the executable's own directory when a `portable` marker file
//! sits next to it, and the OS application data directory otherwise. That keeps
//! a portable copy self-contained on a USB stick while leaving an installed
//! copy behaving like a normal Windows app.
//!
//! Extensions are the one thing that is *not* per profile: they live in a single
//! `extensions` folder beside the executable so an extension installed once is
//! available to every profile.

use std::fs;
use std::path::{Path, PathBuf};

/// `--profile1` .. `--profile99`.
pub const MAX_PROFILE: u32 = 99;

/// Used when the command line names no profile.
pub const DEFAULT_PROFILE: u32 = 1;

/// Presence of any of these next to the executable switches to portable mode.
const PORTABLE_MARKERS: [&str; 2] = ["portable", "portable.txt"];

/// Folders that used to live directly in `<base>` before profiles were
/// mandatory. They are moved into `profile1` on first run.
///
/// `extensions` is deliberately absent: it is shared by every profile and lives
/// in exactly this folder now, so moving it would drag tens of megabytes into a
/// profile and copy them back out again.
const LEGACY_ENTRIES: [&str; 2] = ["webview", "session.json"];

#[derive(Debug, Clone)]
pub struct Profile {
    /// Always in `1..=MAX_PROFILE`; `DEFAULT_PROFILE` when no flag was given.
    pub id: u32,
    pub dir: PathBuf,
    /// Directory holding `profileN` folders. Shared by every profile.
    pub base: PathBuf,
    /// Unpacked extensions, shared by every profile. Sits next to the
    /// executable, with the app data folder as a fallback when that location is
    /// not writable (a machine-wide install under Program Files, say).
    pub extensions: PathBuf,
}

impl Profile {
    /// WebView2 user data folder. Cookies and site storage live here, so each
    /// profile gets its own login session.
    pub fn webview_dir(&self) -> PathBuf {
        self.dir.join("webview")
    }

    pub fn session_path(&self) -> PathBuf {
        self.dir.join("session.json")
    }

    /// Adopt extensions left inside a profile by an older build, then drop the
    /// now-empty folder so only one copy exists.
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

/// Resolve the profile requested on the command line.
///
/// `app_data` is the OS application data directory, used as the base for an
/// installed copy.
pub fn resolve(app_data: Option<&Path>) -> Profile {
    let exe_dir = exe_dir();
    let portable = is_portable();

    let base = if portable {
        exe_dir.clone().unwrap_or_else(|| PathBuf::from("."))
    } else {
        app_data
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."))
    };

    let id = requested().unwrap_or(DEFAULT_PROFILE);
    let dir = base.join(format!("profile{id}"));

    let extensions = extensions_dir(exe_dir.as_deref(), &base);

    let profile = Profile {
        id,
        dir,
        base,
        extensions,
    };

    // Before profiles were mandatory the data sat directly in the base folder.
    // Move it once so an existing login and session survive the change. A
    // profile that already holds a browser profile is left alone.
    if id == DEFAULT_PROFILE && !profile.webview_dir().exists() {
        migrate_legacy(&profile.base, &profile.dir);
    }

    let _ = fs::create_dir_all(&profile.dir);
    let _ = fs::create_dir_all(&profile.extensions);
    profile.adopt_legacy_extensions();
    profile
}

/// `<exe dir>/extensions`, falling back to `<base>/extensions` when the
/// executable sits somewhere the user cannot write to.
fn extensions_dir(exe_dir: Option<&Path>, base: &Path) -> PathBuf {
    let beside_exe = exe_dir.map(|dir| dir.join("extensions"));
    if let Some(candidate) = beside_exe {
        if fs::create_dir_all(&candidate).is_ok() && is_writable(&candidate) {
            return candidate;
        }
    }
    base.join("extensions")
}

/// A directory that exists is not necessarily one this user may write to.
fn is_writable(directory: &Path) -> bool {
    let probe = directory.join(".pake-write-test");
    match fs::write(&probe, b"") {
        Ok(()) => {
            let _ = fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// Move `<base>/{webview,extensions,session.json}` into `<base>/profile1`.
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
        // rename is atomic within a volume; a copy keeps it working otherwise.
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

/// True when a `portable` marker file sits next to the executable, which moves
/// every profile beside the executable instead of into the user's app data.
pub fn is_portable() -> bool {
    exe_dir()
        .map(|dir| PORTABLE_MARKERS.iter().any(|marker| dir.join(marker).exists()))
        .unwrap_or(false)
}

/// True when the command line asked for extensions to be skipped, which is the
/// quick way out if one of them breaks page loads.
pub fn extensions_disabled() -> bool {
    std::env::args()
        .skip(1)
        .any(|arg| arg == "--no-extensions" || arg == "--no-ext")
}

/// The `--profileN` value from the command line, if any.
pub fn requested() -> Option<u32> {
    requested_profile()
}

/// Scan the command line for `--profileN`, `-profileN`, `--profile=N` or
/// `--profile N`. Anything else is ignored so normal Pake flags still work.
fn requested_profile() -> Option<u32> {
    let mut args = std::env::args().skip(1);

    while let Some(arg) = args.next() {
        let trimmed = arg.trim_start_matches('-');
        let Some(rest) = trimmed.strip_prefix("profile") else {
            continue;
        };
        let rest = rest.trim_start_matches(['=', ' ', ':']);

        if rest.is_empty() {
            // `--profile 3`
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
    use super::parse_id;

    #[test]
    fn accepts_only_the_documented_range() {
        assert_eq!(parse_id("1"), Some(1));
        assert_eq!(parse_id("99"), Some(99));
        assert_eq!(parse_id("0"), None);
        assert_eq!(parse_id("100"), None);
        assert_eq!(parse_id("abc"), None);
    }
}
