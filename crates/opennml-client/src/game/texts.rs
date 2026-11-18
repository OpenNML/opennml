//! UI texts from `guids.deu` (`[KEYID n]` lines, `docs/formats/sounddatabase.md`): removing the key marks and the
//! names of the upgrades. Base module without recourse to the unit bar.

use std::collections::{BTreeMap, BTreeSet};

use opennml_formats::gifcon::button_kind;
use opennml_formats::messages::{decode_text_file, parse_text_table};
use opennml_sim::rules::Rules;

/// Path of the text table of the UI (`[KEYID n]` lines, `docs/formats/sounddatabase.md`).
pub const TEXTS_PATH: &str = "data/txt/guids.deu";

/// Text without key marks: `[KEYID n]` is dropped, `[CR]` becomes a space, whitespace is collapsed and the edges are
/// trimmed.
pub fn clean_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('[') {
        out.push_str(&rest[..start]);
        let tail = &rest[start..];
        match tail.find(']') {
            Some(end) => {
                let mark = &tail[1..end];
                if mark == "CR" {
                    out.push(' ');
                } else if !mark.starts_with("KEYID") {
                    out.push_str(&tail[..=end]);
                }
                rest = &tail[end + 1..];
            }
            None => {
                out.push_str(tail);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Texts of the upgrades from the text table `table` of `guids.deu` (cleaned, [`clean_text`]): the GUIDs from
/// `[UPGRADE]` of the project file (`rules.upgrades`) and the contents of the buttons of the kinds `UPGRADE` and
/// `COUNTERUPGRADE` of `toolbar.ini`. Research items are not types; without this table the viewer shows them as
/// "Type n".
pub fn upgrade_texts(table: BTreeMap<i32, String>, rules: &Rules) -> BTreeMap<u32, String> {
    let buttons: BTreeSet<i32> = rules
        .toolbar
        .menus
        .values()
        .flatten()
        .filter(|b| [button_kind::UPGRADE, button_kind::COUNTERUPGRADE].contains(&b.kind()))
        .map(|b| b.msg)
        .collect();
    table
        .into_iter()
        .filter_map(|(k, v)| {
            let guid = u32::try_from(k).ok()?;
            (rules.upgrades.contains(&guid) || buttons.contains(&k)).then(|| (guid, clean_text(&v)))
        })
        .collect()
}

/// [`upgrade_texts`] from `guids.deu` of the shipped data; if the file is missing, the table stays empty
/// (warning).
pub fn load_upgrade_texts(vfs: &opennml_rda::Vfs, rules: &Rules) -> BTreeMap<u32, String> {
    match vfs.read(TEXTS_PATH) {
        Ok(bytes) => upgrade_texts(parse_text_table(&decode_text_file(&bytes)), rules),
        Err(e) => {
            log::warn!("Upgrade names: {TEXTS_PATH} not read: {e:#}");
            BTreeMap::new()
        }
    }
}
