//! Build ID of `nml-game`: sets `NML_BUILD_ID` for `net_start::build_id`.
//!
//! - 12 characters of the commit (`git rev-parse HEAD`);
//! - with changes in the working tree, followed by `-` and 8 hex characters of FNV-1a 32 over the output of
//!   `git diff HEAD` (two modified states match only if the difference is the same; new untracked files do not
//!   count);
//! - without Git (no program, no repository) [`UNKNOWN_BUILD`].
//!
//! `cargo` rebuilds when sources (`crates/`, `apps/`, `Cargo.toml`, `Cargo.lock`) or the state of the repository
//! (`HEAD`, `index`, `refs`, `packed-refs` in the Git folder and in the common Git folder of a worktree) change. A
//! worktree has a file instead of the `.git` folder; the folders are therefore supplied by `git rev-parse --git-dir`
//! and `--git-common-dir`. No separate crate: the script computes FNV itself, `git` runs through
//! `std::process::Command`.
//!
//! Also the program icon of the exe ([`embed_icon`]).

use std::path::{Path, PathBuf};
use std::process::Command;

/// ID without Git; must equal `nml_net::UNKNOWN_BUILD` (Test `net_start::tests::build_id_is_set`).
const UNKNOWN_BUILD: &str = "unbekannt";

/// FNV-1a 32 over `bytes`.
fn fnv1a32(bytes: &[u8]) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for &b in bytes {
        hash = (hash ^ u32::from(b)).wrapping_mul(0x0100_0193);
    }
    hash
}

/// `git <args>` in the folder `dir`; output only on success.
fn git(dir: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .ok()?;
    out.status.success().then_some(out.stdout)
}

/// A path from the output of `git rev-parse` (relative to `dir` if not absolute).
fn git_path(dir: &Path, args: &[&str]) -> Option<PathBuf> {
    let text = String::from_utf8(git(dir, args)?).ok()?;
    let path = PathBuf::from(text.trim());
    Some(if path.is_absolute() {
        path
    } else {
        dir.join(path)
    })
}

fn build_id(dir: &Path) -> String {
    let Some(head) = git(dir, &["rev-parse", "HEAD"]).and_then(|b| String::from_utf8(b).ok())
    else {
        return UNKNOWN_BUILD.to_string();
    };
    let head = head.trim();
    if head.len() < 12 || !head.bytes().all(|b| b.is_ascii_hexdigit()) {
        return UNKNOWN_BUILD.to_string();
    }
    let mut id = head[..12].to_ascii_lowercase();
    match git(dir, &["diff", "HEAD"]) {
        Some(diff) if !diff.is_empty() => id.push_str(&format!("-{:08x}", fnv1a32(&diff))),
        Some(_) => {}
        // If the difference cannot be read, the state is not certain: do not pretend it is clean.
        None => return UNKNOWN_BUILD.to_string(),
    }
    id
}

/// Embeds `assets/icon/opennml.ico` as icon resource 1 in the exe (icon in the Explorer); only when building for
/// Windows. The `.rc` file is created in `OUT_DIR` with an absolute path so that `rc.exe` finds the ICO independent
/// of the working folder. Without a resource compiler the exe stays without an icon (warning during the build); the
/// window sets its icon at run time anyway (`nml_client::app_icon`). Same in `apps/nml-viewer/build.rs`.
fn embed_icon(manifest: &Path) {
    let ico = manifest.join("../../assets/icon/opennml.ico");
    println!("cargo:rerun-if-changed={}", ico.display());
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let ico = std::path::absolute(&ico).expect("path of the ICO cannot be determined");
    // Forward slashes instead of backslashes: no escaping needed in the `.rc` string.
    let ico = ico.to_string_lossy().replace('\\', "/");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR is missing"));
    let rc = out.join("app_icon.rc");
    std::fs::write(&rc, format!("1 ICON \"{ico}\"\n")).expect("app_icon.rc not writable");
    match embed_resource::compile(&rc, embed_resource::NONE) {
        embed_resource::CompilationResult::Ok | embed_resource::CompilationResult::NotWindows => {}
        embed_resource::CompilationResult::NotAttempted(why) => {
            println!("cargo:warning=Program icon not embedded (no resource compiler): {why}");
        }
        failed @ embed_resource::CompilationResult::Failed(_) => {
            panic!("Program icon not embedded: {failed}")
        }
    }
}

fn main() {
    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap_or_default());
    for source in [
        "../../crates",
        "../../apps",
        "../../Cargo.toml",
        "../../Cargo.lock",
    ] {
        println!("cargo:rerun-if-changed={source}");
    }
    let mut git_dirs = Vec::new();
    for flag in ["--git-dir", "--git-common-dir"] {
        if let Some(dir) = git_path(&manifest, &["rev-parse", flag])
            && !git_dirs.contains(&dir)
        {
            git_dirs.push(dir);
        }
    }
    for dir in &git_dirs {
        for name in ["HEAD", "index", "refs", "packed-refs"] {
            let path = dir.join(name);
            if path.exists() {
                println!("cargo:rerun-if-changed={}", path.display());
            }
        }
    }
    println!("cargo:rustc-env=NML_BUILD_ID={}", build_id(&manifest));
    embed_icon(&manifest);
}
