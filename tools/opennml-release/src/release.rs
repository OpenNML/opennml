//! Which package and binary is shipped, read from `cargo metadata`.

use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, bail};
use serde_json::Value;

pub struct Release {
    pub package: String,
    pub version: String,
    pub exe: String,
    pub pdb: String,
    pub manifest_path: PathBuf,
    pub workspace_root: PathBuf,
}

/// Finds the one package with `metadata.dist.dist == true` and its one `bin` target.
pub fn from_metadata(json: &Value) -> anyhow::Result<Release> {
    let packages = json["packages"]
        .as_array()
        .context("metadata has no `packages`")?;
    let shipped: Vec<&Value> = packages
        .iter()
        .filter(|p| p["metadata"]["dist"]["dist"].as_bool() == Some(true))
        .collect();
    let package = match shipped.as_slice() {
        [one] => *one,
        [] => bail!("no package has `package.metadata.dist.dist = true`"),
        _ => bail!(
            "{} packages have `package.metadata.dist.dist = true`, expected one",
            shipped.len()
        ),
    };
    let name = package["name"].as_str().context("package has no name")?;
    let bins: Vec<&str> = package["targets"]
        .as_array()
        .map(|t| t.as_slice())
        .unwrap_or_default()
        .iter()
        .filter(|t| {
            t["kind"]
                .as_array()
                .is_some_and(|k| k.iter().any(|k| k == "bin"))
        })
        .filter_map(|t| t["name"].as_str())
        .collect();
    let [bin] = bins.as_slice() else {
        bail!(
            "package `{name}` has {} binary targets, expected one",
            bins.len()
        );
    };
    Ok(Release {
        package: name.to_string(),
        version: package["version"]
            .as_str()
            .context("package has no version")?
            .to_string(),
        exe: format!("{bin}.exe"),
        pdb: format!("{}.pdb", bin.replace('-', "_")),
        manifest_path: package["manifest_path"]
            .as_str()
            .context("package has no manifest_path")?
            .into(),
        workspace_root: json["workspace_root"]
            .as_str()
            .context("metadata has no workspace_root")?
            .into(),
    })
}

/// Runs `cargo metadata` in the current directory.
pub fn load() -> anyhow::Result<Release> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(cargo)
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .output()
        .context("running cargo metadata")?;
    if !output.status.success() {
        bail!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let json: Value =
        serde_json::from_slice(&output.stdout).context("parsing cargo metadata output")?;
    from_metadata(&json)
}

impl Release {
    /// Paths of the files a release archive contains, with `/` separators.
    pub fn expected_files(&self) -> Vec<String> {
        vec![
            self.exe.clone(),
            self.pdb.clone(),
            "README.txt".into(),
            "LIESMICH.txt".into(),
            "LICENSE.txt".into(),
            "licenses/COPYING.LGPLv2.1.txt".into(),
            "licenses/CC-BY-SA-4.0.txt".into(),
            "licenses/THIRD-PARTY.txt".into(),
        ]
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn package(name: &str, dist: bool, bin: &str) -> Value {
        let mut p = json!({
            "name": name,
            "version": "1.2.3",
            "manifest_path": format!("/w/{name}/Cargo.toml"),
            "targets": [{"kind": ["bin"], "name": bin}, {"kind": ["lib"], "name": name}],
            "metadata": null,
        });
        if dist {
            p["metadata"] = json!({"dist": {"dist": true}});
        }
        p
    }

    fn metadata(packages: Vec<Value>) -> Value {
        json!({"packages": packages, "workspace_root": "/w"})
    }

    #[test]
    fn from_metadata_finds_the_dist_binary() {
        let json = metadata(vec![
            package("opennml-game", true, "opennml-game"),
            package("opennml-viewer", false, "opennml-viewer"),
        ]);
        let r = from_metadata(&json).unwrap();
        assert_eq!(r.package, "opennml-game");
        assert_eq!(r.version, "1.2.3");
        assert_eq!(r.exe, "opennml-game.exe");
        assert_eq!(r.pdb, "opennml_game.pdb");
        assert_eq!(r.expected_files().len(), 8);

        let json = metadata(vec![package("opennml-game", true, "OpenNML")]);
        let r = from_metadata(&json).unwrap();
        assert_eq!(r.exe, "OpenNML.exe");
        assert_eq!(r.pdb, "OpenNML.pdb");

        assert!(from_metadata(&metadata(vec![package("a", false, "a")])).is_err());
        assert!(
            from_metadata(&metadata(vec![
                package("a", true, "a"),
                package("b", true, "b")
            ]))
            .is_err()
        );
    }

    #[test]
    fn metadata_of_this_workspace_has_one_dist_binary() {
        let r = load().unwrap();
        assert_eq!(r.version, env!("CARGO_PKG_VERSION"));
        assert!(r.exe.ends_with(".exe"));
    }

    fn read_toml(path: &std::path::Path) -> toml::Table {
        let text = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
        text.parse()
            .unwrap_or_else(|e| panic!("parsing {}: {e}", path.display()))
    }

    /// The archive `dist` packs must hold the PDB of the shipped binary, built with the `gui` feature.
    #[test]
    fn dist_config_matches_binary() {
        let r = load().unwrap();
        let config = read_toml(&r.workspace_root.join("dist-workspace.toml"));
        let dist = config["dist"].as_table().expect("`[dist]` table");

        let include: Vec<&str> = dist["include"]
            .as_array()
            .expect("`include` array")
            .iter()
            .map(|v| v.as_str().expect("`include` entry is a string"))
            .collect();
        let pdb = format!("target/x86_64-pc-windows-msvc/dist/{}", r.pdb);
        assert_eq!(
            include,
            [
                "target/release-files/README.txt",
                "target/release-files/LIESMICH.txt",
                "target/release-files/LICENSE.txt",
                "target/release-files/licenses",
                pdb.as_str(),
            ]
        );
        assert_eq!(dist["cargo-dist-version"].as_str(), Some("0.34.0"));
        let targets: Vec<&str> = dist["targets"]
            .as_array()
            .expect("`targets` array")
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert_eq!(targets, ["x86_64-pc-windows-msvc"]);
        assert_eq!(dist["auto-includes"].as_bool(), Some(false));

        let manifest = read_toml(&r.manifest_path);
        let features = manifest["package"]["metadata"]["dist"]
            .get("features")
            .and_then(|f| f.as_array())
            .map(|f| f.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>());
        assert_eq!(features, Some(vec!["gui"]));
    }
}
