//! Program icon of the `nml-viewer` exe ([`embed_icon`]).

use std::path::{Path, PathBuf};

/// Embeds `assets/icon/opennml.ico` as icon resource 1 in the exe (icon in the Explorer); only when building for
/// Windows. The `.rc` file is created in `OUT_DIR` with an absolute path so that `rc.exe` finds the ICO independent
/// of the working folder. Without a resource compiler the exe stays without an icon (warning during the build); the
/// window sets its icon at run time anyway (`nml_client::app_icon`). Same in `apps/nml-game/build.rs`.
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
    println!("cargo:rerun-if-changed=build.rs");
    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap_or_default());
    embed_icon(&manifest);
}
