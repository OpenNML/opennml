//! Checks of an unpacked release folder.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use crate::pe::read_pe;
use crate::release::Release;

/// System DLLs the exe may import (compared without case), plus every `api-ms-win-*.dll`.
pub const ALLOWED_IMPORTS: &[&str] = &[
    "kernel32.dll",
    "user32.dll",
    "gdi32.dll",
    "dxgi.dll",
    "dsound.dll",
    "mmdevapi.dll",
    "opengl32.dll",
    "ws2_32.dll",
    "setupapi.dll",
    "combase.dll",
    "ole32.dll",
    "oleaut32.dll",
    "shell32.dll",
    "uxtheme.dll",
    "dwmapi.dll",
    "imm32.dll",
    "advapi32.dll",
    "bcrypt.dll",
    "bcryptprimitives.dll",
    "ntdll.dll",
    "comctl32.dll",
    "comdlg32.dll",
    "shlwapi.dll",
    "winmm.dll",
    "version.dll",
    "propsys.dll",
    "d3d12.dll",
    "d3dcompiler_47.dll",
];

/// One finding per imported DLL that is not allowed.
pub fn import_problems(imports: &[String]) -> Vec<String> {
    imports
        .iter()
        .filter(|name| {
            let lower = name.to_ascii_lowercase();
            let api_set = lower.starts_with("api-ms-win-") && lower.ends_with(".dll");
            !api_set && !ALLOWED_IMPORTS.contains(&lower.as_str())
        })
        .map(|name| format!("{name}: import is not an allowed system DLL"))
        .collect()
}

fn collect_files(root: &Path, dir: &Path, out: &mut BTreeSet<String>) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            collect_files(root, &path, out)?;
        } else if let Ok(rel) = path.strip_prefix(root) {
            let parts: Vec<_> = rel.iter().map(|p| p.to_string_lossy()).collect();
            out.insert(parts.join("/"));
        }
    }
    Ok(())
}

fn check_text(name: &str, text: &[u8], release: &Release, findings: &mut Vec<String>) {
    let Ok(text) = std::str::from_utf8(text) else {
        findings.push(format!("{name}: not valid UTF-8"));
        return;
    };
    if text.starts_with('\u{feff}') {
        findings.push(format!("{name}: starts with a UTF-8 BOM, expected none"));
    }
    if !text.contains(&release.version) {
        findings.push(format!(
            "{name}: does not mention version {}",
            release.version
        ));
    }
    if !text.contains(&release.exe) {
        findings.push(format!("{name}: does not mention {}", release.exe));
    }
    let bytes = text.as_bytes();
    let bare_lf = bytes
        .iter()
        .enumerate()
        .any(|(i, &b)| b == b'\n' && (i == 0 || bytes[i - 1] != b'\r'));
    let bare_cr = bytes
        .iter()
        .enumerate()
        .any(|(i, &b)| b == b'\r' && bytes.get(i + 1) != Some(&b'\n'));
    if bare_lf || bare_cr {
        findings.push(format!("{name}: line endings are not all CRLF"));
    }
}

/// Checks the folder against the release; returns one line per finding (empty when fine).
pub fn check_folder(dir: &Path, release: &Release) -> Vec<String> {
    let mut findings = Vec::new();
    let mut present = BTreeSet::new();
    if let Err(err) = collect_files(dir, dir, &mut present) {
        return vec![format!("{}: cannot list folder: {err}", dir.display())];
    }
    let expected = release.expected_files();
    for name in &expected {
        if !present.contains(name) {
            findings.push(format!("{name}: file is missing"));
        }
    }
    for name in &present {
        if !expected.contains(name) {
            findings.push(format!("{name}: unexpected file"));
        }
    }

    if present.contains(&release.exe) {
        let exe = &release.exe;
        match fs::read(dir.join(exe)) {
            Err(err) => findings.push(format!("{exe}: cannot read: {err}")),
            Ok(bytes) => match read_pe(&bytes) {
                Err(err) => findings.push(format!("{exe}: {err:#}")),
                Ok(info) => {
                    if info.machine != 0x8664 {
                        findings.push(format!(
                            "{exe}: machine is {:#06x}, expected 0x8664 (x86-64)",
                            info.machine
                        ));
                    }
                    if info.subsystem != 2 {
                        findings.push(format!(
                            "{exe}: subsystem is {}, expected 2 (Windows GUI)",
                            info.subsystem
                        ));
                    }
                    findings.extend(
                        import_problems(&info.imports)
                            .into_iter()
                            .map(|f| format!("{exe}: {f}")),
                    );
                }
            },
        }
    }

    for name in ["README.txt", "LIESMICH.txt"] {
        if present.contains(name) {
            match fs::read(dir.join(name)) {
                Ok(text) => check_text(name, &text, release, &mut findings),
                Err(err) => findings.push(format!("{name}: cannot read: {err}")),
            }
        }
    }
    findings
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn import_problems_flags_the_vc_runtime() {
        assert!(
            import_problems(&names(&["KERNEL32.dll", "api-ms-win-crt-heap-l1-1-0.dll"])).is_empty()
        );
        for bad in ["VCRUNTIME140.dll", "ucrtbase.dll", "foo.dll"] {
            let problems = import_problems(&names(&[bad]));
            assert_eq!(problems.len(), 1, "{bad}");
            assert!(problems[0].contains(bad));
        }
    }

    #[test]
    fn check_folder_reports_missing_and_extra_files() {
        let release = Release {
            package: "opennml-game".into(),
            version: "0.9.0".into(),
            exe: "OpenNML.exe".into(),
            pdb: "OpenNML.pdb".into(),
            manifest_path: Default::default(),
            workspace_root: Default::default(),
        };
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("licenses")).unwrap();
        for name in release.expected_files() {
            let path = dir.path().join(&name);
            if name == release.exe {
                fs::copy(std::env::current_exe().unwrap(), path).unwrap();
            } else if name == "README.txt" {
                fs::write(path, "OpenNML 0.9.0\r\nOpenNML.exe\r\n").unwrap();
            } else if name != "LIESMICH.txt" {
                fs::write(path, "x\r\n").unwrap();
            }
        }
        fs::write(dir.path().join("extra.txt"), "x").unwrap();

        let findings = check_folder(dir.path(), &release);
        let has = |part: &str| findings.iter().any(|f| f.contains(part));
        assert!(has("LIESMICH.txt: file is missing"), "{findings:?}");
        assert!(has("extra.txt: unexpected file"), "{findings:?}");
        assert!(has("subsystem is 3"), "{findings:?}");
        assert!(!has("README.txt"), "{findings:?}");
    }

    fn text_findings(text: &[u8]) -> Vec<String> {
        let release = Release {
            package: "opennml-game".into(),
            version: "0.9.0".into(),
            exe: "OpenNML.exe".into(),
            pdb: "OpenNML.pdb".into(),
            manifest_path: Default::default(),
            workspace_root: Default::default(),
        };
        let mut findings = Vec::new();
        check_text("README.txt", text, &release, &mut findings);
        findings
    }

    #[test]
    fn check_text_reports_readme_problems() {
        assert!(text_findings(b"OpenNML 0.9.0 OpenNML.exe\r\nx\r\n").is_empty());

        let findings = text_findings(b"OpenNML 0.9.0 OpenNML.exe\nx\r\n");
        assert!(findings.iter().any(|f| f.contains("CRLF")), "{findings:?}");

        let findings = text_findings(b"OpenNML OpenNML.exe\r\n");
        assert!(
            findings.iter().any(|f| f.contains("version 0.9.0")),
            "{findings:?}"
        );

        let findings = text_findings(b"OpenNML 0.9.0\r\n");
        assert!(
            findings.iter().any(|f| f.contains("OpenNML.exe")),
            "{findings:?}"
        );

        let findings = text_findings("\u{feff}OpenNML 0.9.0 OpenNML.exe\r\n".as_bytes());
        assert!(findings.iter().any(|f| f.contains("BOM")), "{findings:?}");
    }
}
