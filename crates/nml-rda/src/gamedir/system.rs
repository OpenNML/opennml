//! Locations from the Windows registry and the program folders.

use std::path::PathBuf;

use super::Candidate;

/// Does the display name of an uninstall entry belong to No Man's Land?
pub fn uninstall_matches(display_name: &str) -> bool {
    display_name
        .to_lowercase()
        .replace('\u{2019}', "'")
        .contains("no man's land")
}

/// The usual folders under the program folder roots, in order for each root.
pub fn program_files_candidates(roots: &[PathBuf]) -> Vec<PathBuf> {
    const SUBDIRS: [&[&str]; 4] = [
        &["No Man's Land"],
        &["CDV", "No Man's Land"],
        &["CDV Software", "No Man's Land"],
        &["Related Designs", "No Man's Land"],
    ];
    let mut out = Vec::new();
    for root in roots {
        for parts in SUBDIRS {
            out.push(parts.iter().fold(root.clone(), |p, part| p.join(part)));
        }
    }
    out
}

/// Locations from Steam, uninstall entries, and program folders (Windows only, otherwise empty).
/// The registry is only read; errors are silently ignored.
pub(crate) fn system_candidates() -> Vec<Candidate> {
    #[cfg(windows)]
    {
        windows::collect()
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

#[cfg(windows)]
mod windows {
    use std::path::PathBuf;

    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ};
    use winreg::{HKEY, RegKey};

    use super::{program_files_candidates, uninstall_matches};
    use crate::gamedir::steam::steam_candidates;
    use crate::gamedir::{Candidate, Source};

    fn read_string(root: HKEY, path: &str, name: &str) -> Option<String> {
        let key = RegKey::predef(root)
            .open_subkey_with_flags(path, KEY_READ)
            .ok()?;
        key.get_value::<String, _>(name).ok()
    }

    fn uninstall_locations(root: HKEY, path: &str) -> Vec<PathBuf> {
        let Ok(parent) = RegKey::predef(root).open_subkey_with_flags(path, KEY_READ) else {
            return Vec::new();
        };
        let mut names: Vec<String> = parent.enum_keys().filter_map(Result::ok).collect();
        names.sort();
        let mut out = Vec::new();
        for name in names {
            let Ok(sub) = parent.open_subkey_with_flags(&name, KEY_READ) else {
                continue;
            };
            let Ok(display) = sub.get_value::<String, _>("DisplayName") else {
                continue;
            };
            if !uninstall_matches(&display) {
                continue;
            }
            if let Ok(location) = sub.get_value::<String, _>("InstallLocation") {
                let location = location.trim().trim_matches('"').trim();
                if !location.is_empty() {
                    out.push(PathBuf::from(location));
                }
            }
        }
        out
    }

    pub(super) fn collect() -> Vec<Candidate> {
        let mut out = Vec::new();
        let mut steam_roots = Vec::new();
        if let Some(p) = read_string(HKEY_CURRENT_USER, "Software\\Valve\\Steam", "SteamPath") {
            steam_roots.push(PathBuf::from(p));
        }
        if let Some(p) = read_string(
            HKEY_LOCAL_MACHINE,
            "SOFTWARE\\WOW6432Node\\Valve\\Steam",
            "InstallPath",
        ) {
            steam_roots.push(PathBuf::from(p));
        }
        for root in steam_roots {
            for path in steam_candidates(&root) {
                out.push(Candidate {
                    path,
                    source: Source::Steam,
                });
            }
        }
        let uninstall = "\\Microsoft\\Windows\\CurrentVersion\\Uninstall";
        let locations = [
            (HKEY_LOCAL_MACHINE, format!("SOFTWARE{uninstall}")),
            (
                HKEY_LOCAL_MACHINE,
                format!("SOFTWARE\\WOW6432Node{uninstall}"),
            ),
            (HKEY_CURRENT_USER, format!("Software{uninstall}")),
        ];
        for (root, path) in locations {
            for path in uninstall_locations(root, &path) {
                out.push(Candidate {
                    path,
                    source: Source::Uninstall,
                });
            }
        }
        let roots: Vec<PathBuf> = ["ProgramFiles(x86)", "ProgramFiles"]
            .iter()
            .filter_map(std::env::var_os)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .collect();
        for path in program_files_candidates(&roots) {
            out.push(Candidate {
                path,
                source: Source::ProgramFiles,
            });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uninstall_names() {
        assert!(uninstall_matches("No Man's Land"));
        assert!(uninstall_matches("NO MAN’S LAND v1.1"));
        assert!(!uninstall_matches("No Man's Sky"));
    }

    #[test]
    fn program_files_order() {
        let roots = [PathBuf::from("A"), PathBuf::from("B")];
        let mut expected = Vec::new();
        for root in ["A", "B"] {
            let root = PathBuf::from(root);
            expected.push(root.join("No Man's Land"));
            expected.push(root.join("CDV").join("No Man's Land"));
            expected.push(root.join("CDV Software").join("No Man's Land"));
            expected.push(root.join("Related Designs").join("No Man's Land"));
        }
        assert_eq!(program_files_candidates(&roots), expected);
    }

    #[test]
    fn system_candidates_do_not_panic() {
        let _ = system_candidates();
    }
}
