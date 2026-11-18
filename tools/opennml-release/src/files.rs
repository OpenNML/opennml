//! Generates the readme and license files of the release archive.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, bail};

use crate::release::Release;

const README: &str = include_str!("../templates/README.txt");
const LIESMICH: &str = include_str!("../templates/LIESMICH.txt");
const ABOUT_TOML: &str = "tools/opennml-release/about.toml";
const ABOUT_TEMPLATE: &str = "tools/opennml-release/about.hbs";
const TARGET: &str = "x86_64-pc-windows-msvc";
const INSTALL_HINT: &str = "cargo install --locked cargo-about --version 0.9.3 --features cli";

/// Converts every line ending (LF or CRLF) to CRLF.
fn to_crlf(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\n', "\r\n")
}

/// Replaces `{version}`, `{commit}` (first 12 characters) and `{exe}`; any other `{...}` is an error.
/// The result has CRLF line endings.
pub fn fill_template(text: &str, release: &Release, commit: &str) -> anyhow::Result<String> {
    let Some(short) = commit.get(..12) else {
        bail!("commit hash `{commit}` has fewer than 12 characters");
    };
    let mut out = String::with_capacity(text.len() + 64);
    let mut rest = text;
    while let Some(open) = rest.find(['{', '}']) {
        out.push_str(&rest[..open]);
        let tail = &rest[open..];
        let Some(inner) = tail.strip_prefix('{') else {
            bail!("unmatched `}}` in template");
        };
        let close = inner.find('}').context("unterminated `{` in template")?;
        let name = &inner[..close];
        match name {
            "version" => out.push_str(&release.version),
            "commit" => out.push_str(short),
            "exe" => out.push_str(&release.exe),
            other => bail!("unknown placeholder `{{{other}}}` in template"),
        }
        rest = &inner[close + 1..];
    }
    out.push_str(rest);
    Ok(to_crlf(&out))
}

fn cargo_about_installed() -> bool {
    Command::new("cargo")
        .args(["about", "--version"])
        .output()
        .is_ok_and(|o| o.status.success())
}

const OVERVIEW_BEGIN: &str = "@@OVERVIEW-BEGIN@@";
const OVERVIEW_END: &str = "@@OVERVIEW-END@@";

/// Replaces the marked block of `about.hbs` (lines `name version | license id`, one per license entry) by one
/// line per crate with all its license ids, and checks that these are exactly the crates listed below it.
fn group_overview(text: &str) -> anyhow::Result<String> {
    let text = text.replace("\r\n", "\n");
    let begin = text
        .find(OVERVIEW_BEGIN)
        .context("overview start marker missing")?;
    let end = text
        .find(OVERVIEW_END)
        .context("overview end marker missing")?;
    if end < begin {
        bail!("overview markers are in the wrong order");
    }
    let block = &text[begin + OVERVIEW_BEGIN.len()..end];
    let mut crates: BTreeMap<(&str, &str), BTreeSet<&str>> = BTreeMap::new();
    for line in block.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let (krate, license) = line
            .split_once(" | ")
            .with_context(|| format!("bad overview line `{line}`"))?;
        let (name, version) = krate
            .split_once(' ')
            .with_context(|| format!("bad overview line `{line}`"))?;
        crates.entry((name, version)).or_default().insert(license);
    }
    let after = &text[end + OVERVIEW_END.len()..];
    let listed: BTreeSet<(&str, &str)> = after
        .lines()
        .filter_map(|l| l.strip_prefix("  - "))
        .filter_map(|l| {
            let mut parts = l.split(' ');
            Some((parts.next()?, parts.next()?))
        })
        .collect();
    let overview: BTreeSet<(&str, &str)> = crates.keys().copied().collect();
    if overview != listed {
        bail!(
            "overview has {} crates but the license texts list {}",
            overview.len(),
            listed.len()
        );
    }
    let mut lines = String::new();
    for ((name, version), licenses) in &crates {
        let licenses: Vec<&str> = licenses.iter().copied().collect();
        lines.push_str(&format!("{name} {version}  ({})\n", licenses.join(", ")));
    }
    let head = text[..begin].trim_end_matches('\n');
    Ok(format!(
        "{head}\n\n{lines}{}",
        after.strip_prefix('\n').unwrap_or(after)
    ))
}

fn third_party(release: &Release) -> anyhow::Result<String> {
    // cargo-about refuses to write to stdout when any parent process is PowerShell (the default shell of
    // Windows runners in GitHub Actions), so it writes to a file.
    let about_out =
        std::env::temp_dir().join(format!("opennml-release-about-{}.txt", std::process::id()));
    let output = Command::new("cargo")
        .current_dir(&release.workspace_root)
        .args([
            "about",
            "generate",
            "--config",
            ABOUT_TOML,
            "--manifest-path",
        ])
        .arg(&release.manifest_path)
        .args([
            "--target",
            TARGET,
            "--features",
            "gui",
            "--fail",
            "--locked",
            "--output-file",
        ])
        .arg(&about_out)
        .arg(ABOUT_TEMPLATE)
        .output()
        .context("running cargo about generate")?;
    let bytes = fs::read(&about_out);
    let _ = fs::remove_file(&about_out);
    if !output.status.success() {
        bail!(
            "cargo about generate failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let bytes = bytes.with_context(|| format!("reading {}", about_out.display()))?;
    let text = String::from_utf8(bytes).context("cargo about output is not UTF-8")?;
    let mut text = group_overview(&text)?;

    let rustc = Command::new("rustc")
        .current_dir(&release.workspace_root)
        .arg("--version")
        .output()
        .context("running rustc --version")?;
    if !rustc.status.success() {
        bail!("rustc --version failed");
    }
    let rustc = String::from_utf8_lossy(&rustc.stdout).trim().to_string();
    text.push_str(&format!(
        "\n\nRust standard library\n=====================\n\n\
         The programs are built with {rustc} and statically contain parts of its\n\
         standard library.\n\
         License: MIT OR Apache-2.0\n\
         Copyright: The Rust Project Developers\n\
         \n\nFFmpeg\n======\n\n\
         The video decoder `opennml-bink` is a port from FFmpeg (LGPL-2.1-or-later). The\n\
         license text is in licenses/COPYING.LGPLv2.1.txt; the origin is described in\n\
         crates/opennml-bink/README.md in the source code.\n"
    ));
    Ok(to_crlf(&text))
}

/// Refuses an output folder that would destroy something else: the workspace root, an ancestor of it, or an
/// existing non-empty folder that is not a previous output (contains `README.txt` and `licenses`).
fn check_out_dir(out: &Path, root: &Path) -> anyhow::Result<()> {
    if !out.exists() {
        return Ok(());
    }
    let canonical = |p: &Path| {
        p.canonicalize()
            .with_context(|| format!("resolving {}", p.display()))
    };
    let out_abs = canonical(out)?;
    if canonical(root)?.starts_with(&out_abs) {
        bail!(
            "refusing to clear {}: it is the workspace root or contains it",
            out.display()
        );
    }
    let empty = fs::read_dir(out)
        .with_context(|| format!("reading {}", out.display()))?
        .next()
        .is_none();
    let previous_output = out.join("README.txt").is_file() && out.join("licenses").is_dir();
    if !empty && !previous_output {
        bail!(
            "refusing to clear {}: it is not empty and not a previous output",
            out.display()
        );
    }
    Ok(())
}

/// Clears `out` after `check_out_dir`.
fn prepare_out_dir(out: &Path, root: &Path) -> anyhow::Result<()> {
    check_out_dir(out, root)?;
    if out.exists() {
        fs::remove_dir_all(out).with_context(|| format!("clearing {}", out.display()))?;
    }
    Ok(())
}

/// Writes the readme and license files into `out` (cleared first).
pub fn write_release_files(release: &Release, commit: &str, out: &Path) -> anyhow::Result<()> {
    let root = &release.workspace_root;
    check_out_dir(out, root)?;
    if !cargo_about_installed() {
        bail!("`cargo about` is not installed; run `{INSTALL_HINT}`");
    }
    let read =
        |rel: &str| fs::read_to_string(root.join(rel)).with_context(|| format!("reading {rel}"));
    let readme = fill_template(README, release, commit)?;
    let liesmich = fill_template(LIESMICH, release, commit)?;
    let license = to_crlf(&read("LICENSE")?);
    let lgpl = to_crlf(&format!(
        "This is the license of the video decoder opennml-bink, a port from FFmpeg.\n\n{}",
        read("crates/opennml-bink/COPYING.LGPLv2.1")?
    ));
    let cc = to_crlf(&read("LICENSE-CC-BY-SA-4.0")?);
    let third = third_party(release)?;

    prepare_out_dir(out, root)?;
    fs::create_dir_all(out.join("licenses"))
        .with_context(|| format!("creating {}", out.display()))?;
    let write = |rel: &str, text: &str| {
        fs::write(out.join(rel), text.as_bytes()).with_context(|| format!("writing {rel}"))
    };
    write("README.txt", &readme)?;
    write("LIESMICH.txt", &liesmich)?;
    write("LICENSE.txt", &license)?;
    write("licenses/COPYING.LGPLv2.1.txt", &lgpl)?;
    write("licenses/CC-BY-SA-4.0.txt", &cc)?;
    write("licenses/THIRD-PARTY.txt", &third)?;
    Ok(())
}

/// The commit hash of `HEAD` in `dir`.
pub fn git_commit(dir: &Path) -> anyhow::Result<String> {
    let output = Command::new("git")
        .current_dir(dir)
        .args(["rev-parse", "HEAD"])
        .output()
        .context("running git rev-parse HEAD")?;
    if !output.status.success() {
        bail!("git rev-parse HEAD failed");
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release() -> Release {
        Release {
            package: "opennml-game".into(),
            version: "0.9.0".into(),
            exe: "OpenNML.exe".into(),
            pdb: "OpenNML.pdb".into(),
            manifest_path: Default::default(),
            workspace_root: Default::default(),
        }
    }

    const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

    #[test]
    fn fill_template_replaces_placeholders_and_uses_crlf() {
        let r = release();
        assert_eq!(
            fill_template("A {version}\nB {commit} {exe}\n", &r, COMMIT).unwrap(),
            "A 0.9.0\r\nB 0123456789ab OpenNML.exe\r\n"
        );
        assert_eq!(
            fill_template("A {version}\r\nB\r\n", &r, COMMIT).unwrap(),
            "A 0.9.0\r\nB\r\n"
        );
        assert!(fill_template("{foo}", &r, COMMIT).is_err());
        assert!(fill_template("{version", &r, COMMIT).is_err());
        assert!(fill_template("}", &r, COMMIT).is_err());
        assert!(fill_template("{commit}", &r, "0123").is_err());
    }

    #[test]
    fn templates_use_only_known_placeholders() {
        let r = release();
        for (name, template) in [("README.txt", README), ("LIESMICH.txt", LIESMICH)] {
            let text =
                fill_template(template, &r, COMMIT).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(text.contains("OpenNML.log"), "{name}");
            assert!(text.contains("%LOCALAPPDATA%\\OpenNML"), "{name}");
            assert!(text.contains("0.9.0"), "{name}");
            assert!(text.contains("OpenNML.exe"), "{name}");
            assert!(!text.contains("\r\r"), "{name}");
            assert!(!text.starts_with('\u{feff}'), "{name}");
        }
    }

    #[test]
    fn group_overview_makes_one_line_per_crate() {
        let text = "head\n@@OVERVIEW-BEGIN@@\nb 1.0 | MIT\na 2.0 | Zlib\nb 1.0 | Unicode-3.0\nb 1.0 | MIT\n\
                    @@OVERVIEW-END@@\n\nTEXT\n  - a 2.0 (https://x)\n  - b 1.0\n    Source code: y\n";
        let out = group_overview(text).unwrap();
        assert_eq!(
            out,
            "head\n\na 2.0  (Zlib)\nb 1.0  (MIT, Unicode-3.0)\n\nTEXT\n  - a 2.0 (https://x)\n  - b 1.0\n    Source code: y\n"
        );
        // A crate in the overview that the texts do not list (or the reverse) is an error.
        let missing = text.replace("  - b 1.0\n", "");
        assert!(group_overview(&missing).is_err());
        assert!(group_overview("no markers").is_err());
    }

    #[test]
    fn out_dir_guard_protects_the_checkout() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("work").join("repo");
        fs::create_dir_all(&root).unwrap();

        // The workspace root and its ancestors are refused.
        assert!(prepare_out_dir(&root, &root).is_err());
        assert!(prepare_out_dir(&root.join(".."), &root).is_err());
        assert!(prepare_out_dir(tmp.path(), &root).is_err());
        assert!(root.exists());

        // A foreign non-empty folder is refused and left alone.
        let foreign = tmp.path().join("foreign");
        fs::create_dir(&foreign).unwrap();
        fs::write(foreign.join("keep.txt"), "x").unwrap();
        assert!(prepare_out_dir(&foreign, &root).is_err());
        assert!(foreign.join("keep.txt").exists());

        // A previous output is cleared.
        let previous = root.join("target").join("release-files");
        fs::create_dir_all(previous.join("licenses")).unwrap();
        fs::write(previous.join("README.txt"), "x").unwrap();
        prepare_out_dir(&previous, &root).unwrap();
        assert!(!previous.exists());

        // An empty folder and a missing folder are fine.
        let empty = tmp.path().join("empty");
        fs::create_dir(&empty).unwrap();
        prepare_out_dir(&empty, &root).unwrap();
        prepare_out_dir(&tmp.path().join("missing"), &root).unwrap();
    }

    #[test]
    fn write_release_files_without_cargo_about_fails_cleanly() {
        if cargo_about_installed() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let err = write_release_files(&release(), COMMIT, dir.path()).unwrap_err();
        assert!(format!("{err:#}").contains(INSTALL_HINT), "{err:#}");
    }
}
