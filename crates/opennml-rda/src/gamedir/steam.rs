//! Reading Steam libraries and the app manifest.

use std::path::{Path, PathBuf};

/// The Steam app ID of No Man's Land.
pub const STEAM_APP_ID: u32 = 3863620;

/// A token of the Valve text format.
enum Token {
    Text(String),
    Open,
    Close,
}

fn tokenize(text: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' => tokens.push(Token::Open),
            '}' => tokens.push(Token::Close),
            '/' if chars.peek() == Some(&'/') => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
            }
            '"' => {
                let mut value = String::new();
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' => match chars.next() {
                            Some('\\') => value.push('\\'),
                            Some('"') => value.push('"'),
                            Some('n') => value.push('\n'),
                            Some('t') => value.push('\t'),
                            Some(other) => {
                                value.push('\\');
                                value.push(other);
                            }
                            None => value.push('\\'),
                        },
                        other => value.push(other),
                    }
                }
                tokens.push(Token::Text(value));
            }
            _ => {}
        }
    }
    tokens
}

/// All values for `"key" "value"` in the Valve text format (keys are case-insensitive).
/// Block braces are ignored (keys are found at any depth); comments (`//`) are skipped; `\\` and `\"` in the
/// value are unescaped.
pub fn vdf_values(text: &str, key: &str) -> Vec<String> {
    let tokens = tokenize(text);
    let mut out = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        match (&tokens[i], tokens.get(i + 1)) {
            (Token::Text(k), Some(Token::Text(v))) => {
                if k.eq_ignore_ascii_case(key) {
                    out.push(v.clone());
                }
                i += 2;
            }
            _ => i += 1,
        }
    }
    out
}

/// Possible installation folders from the Steam libraries under `steam_root`: `steam_root` itself
/// and the `path` values from `steamapps/libraryfolders.vdf`. For each library, the `installdir` of
/// the app manifest counts. Missing or unreadable files are silently ignored.
pub fn steam_candidates(steam_root: &Path) -> Vec<PathBuf> {
    let mut libraries = vec![steam_root.to_path_buf()];
    if let Ok(text) = std::fs::read_to_string(steam_root.join("steamapps/libraryfolders.vdf")) {
        for value in vdf_values(&text, "path") {
            let path = PathBuf::from(value);
            if !libraries.contains(&path) {
                libraries.push(path);
            }
        }
    }
    let manifest = format!("steamapps/appmanifest_{STEAM_APP_ID}.acf");
    let mut out = Vec::new();
    for library in libraries {
        let Ok(text) = std::fs::read_to_string(library.join(&manifest)) else {
            continue;
        };
        if let Some(dir) = vdf_values(&text, "installdir")
            .into_iter()
            .find(|d| !d.trim().is_empty())
        {
            out.push(library.join("steamapps/common").join(dir));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIBS: &str = "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"C:\\\\Program Files (x86)\\\\Steam\"\n\t\t\"apps\" { \"3863620\" \"123\" }\n\t}\n\t\"1\" { \"PATH\" \"E:\\\\Games\\\\Steam\" }\n}\n";

    #[test]
    fn vdf_reads_paths_case_insensitive() {
        assert_eq!(
            vdf_values(LIBS, "path"),
            vec!["C:\\Program Files (x86)\\Steam", "E:\\Games\\Steam"]
        );
    }

    #[test]
    fn vdf_skips_comments_and_unescapes_quotes() {
        let text = "// \"path\" \"commented\"\n\"a\" { \"installdir\" \"Say \\\"Hi\\\"\" }\n";
        assert_eq!(vdf_values(text, "installdir"), vec!["Say \"Hi\""]);
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn escaped(path: &Path) -> String {
        path.to_string_lossy().replace('\\', "\\\\")
    }

    #[test]
    fn steam_finds_install_via_manifest() {
        let t = tempfile::tempdir().unwrap();
        let steam = t.path().join("steam");
        let lib2 = t.path().join("lib2");
        write(
            &steam.join("steamapps/libraryfolders.vdf"),
            &format!(
                "\"libraryfolders\" {{ \"0\" {{ \"path\" \"{}\" }} \"1\" {{ \"path\" \"{}\" }} }}",
                escaped(&steam),
                escaped(&lib2)
            ),
        );
        write(
            &lib2.join("steamapps/appmanifest_3863620.acf"),
            "\"AppState\"\n{\n\t\"appid\"\t\"3863620\"\n\t\"installdir\"\t\"No Man's Land\"\n}\n",
        );
        assert_eq!(
            steam_candidates(&steam),
            vec![lib2.join("steamapps/common/No Man's Land")]
        );
    }

    #[test]
    fn steam_skips_manifest_without_installdir() {
        let t = tempfile::tempdir().unwrap();
        let steam = t.path().join("steam");
        let lib2 = t.path().join("lib2");
        write(
            &steam.join("steamapps/libraryfolders.vdf"),
            &format!(
                "\"libraryfolders\" {{ \"1\" {{ \"path\" \"{}\" }} \"2\" {{ \"path\" \"{}\" }} }}",
                escaped(&lib2),
                escaped(&t.path().join("lib3"))
            ),
        );
        write(
            &steam.join("steamapps/appmanifest_3863620.acf"),
            "\"AppState\" { \"appid\" \"3863620\" \"Other\" \"x\" }",
        );
        write(
            &lib2.join("steamapps/appmanifest_3863620.acf"),
            "\"AppState\" { \"appid\" \"3863620\" }",
        );
        assert!(steam_candidates(&steam).is_empty());
        assert!(steam_candidates(&t.path().join("missing")).is_empty());
    }
}
