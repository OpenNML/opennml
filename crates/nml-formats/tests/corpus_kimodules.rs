//! Reads all AI modules of the shipped data and checks the invariants from `docs/formats/kimodule.md`.

use std::collections::{BTreeMap, BTreeSet};

use nml_formats::kimodule::{DB_PATH, DROP, KiModule, MODULE_DIR, module_path, parse, parse_db};
use nml_rda::testutil::corpus_game_dir;
use nml_rda::{Vfs, normalize_path};

/// Kind, type and index of a variable address.
fn split(addr: u32) -> (u32, u32, u32) {
    ((addr >> 24) & 0xF, (addr >> 16) & 0xFF, addr & 0xFFFF)
}

/// Length of the range in which the address lies (0 without a range).
fn range_len(m: &KiModule, addr: u32) -> u32 {
    let (k, t, _) = split(addr);
    m.ranges
        .iter()
        .find(|&&(id, _)| (split(id).0, split(id).1) == (k, t))
        .map_or(0, |&(_, len)| len)
}

#[test]
fn kimodules_parse_with_the_documented_invariants() {
    let Some(dir) = corpus_game_dir() else {
        eprintln!("Original installation not found - corpus test skipped");
        return;
    };
    let vfs = Vfs::open_game_dir(&dir).unwrap();
    let prefix = format!("{MODULE_DIR}/");
    let paths: Vec<String> = vfs
        .files()
        .map(|(_, e)| normalize_path(&e.path))
        .filter(|p| p.starts_with(&prefix) && p.ends_with(".mod"))
        .collect();
    assert_eq!(paths.len(), 291);

    let mut modules = BTreeMap::new();
    for path in &paths {
        let id: u32 = path[prefix.len()..path.len() - 4].parse().unwrap();
        assert_eq!(&module_path(id), path);
        let data = vfs.read(path).unwrap_or_else(|e| panic!("{path}: {e}"));
        let m = parse(&data).unwrap_or_else(|e| panic!("{path}: {e}"));
        modules.insert(id, m);
    }

    let mut max_range_len = 0;
    let mut jump_to_end = 0;
    let mut callb = 0;
    let mut callb_ids = BTreeSet::new();
    let mut callm = BTreeSet::new();
    let mut spawn = BTreeSet::new();
    // (module, count) of the STORE/STORE3 on kind >= 2 and of the LOAD on kind 1.
    let mut store_on_list: BTreeMap<u32, u32> = BTreeMap::new();
    let mut load_on_ref: BTreeMap<u32, u32> = BTreeMap::new();
    for (&id, m) in &modules {
        let ctx = format!("module {id}");
        assert_eq!(
            m.blocks,
            [
                "KIMODULE", "HEADER", "VARIABLE", "CONSTDAT", "CODEDATA", "VARMAP", "DEBUGGER"
            ],
            "{ctx}"
        );
        assert_eq!(m.header, Some(1), "{ctx}");
        for &(rid, len) in &m.ranges {
            max_range_len = max_range_len.max(len);
            let (k, t, i) = split(rid);
            assert!(i == 0 && k <= 3 && t <= 4, "{ctx}: range {rid:#x}");
        }
        for &(addr, _) in &m.inits {
            let (k, _, i) = split(addr);
            assert!(
                k == 0 && i < range_len(m, addr),
                "{ctx}: initial value {addr:#x}"
            );
        }
        let len = m.code.len() as u32;
        for (i, &w) in m.code.iter().enumerate() {
            let (op, a) = (w >> 28, w & 0x0FFF_FFFF);
            assert!(op <= 8, "{ctx}: opcode {op} at {i}");
            match op {
                0 | 1 | 3 if a != DROP => {
                    let (k, _, ix) = split(a);
                    assert!(ix < range_len(m, a), "{ctx}: variable {a:#x} at {i}");
                    if op != 0 && k >= 2 {
                        *store_on_list.entry(id).or_default() += 1;
                    }
                    if op == 0 && k == 1 {
                        *load_on_ref.entry(id).or_default() += 1;
                    }
                }
                2 => {
                    let k = split(a).0;
                    assert_ne!(k, 0, "{ctx}: PUSHA kind 0 at {i}");
                    let next = m.code.get(i + 1).map(|&n| n >> 28);
                    assert!(
                        !(k >= 2 && next == Some(1)),
                        "{ctx}: PUSHA of a list directly before STORE at {i}"
                    );
                }
                4 => {
                    callb += 1;
                    callb_ids.insert(a);
                }
                5 => {
                    callm.insert(a);
                }
                6 => {
                    spawn.insert(a);
                }
                7..=9 => {
                    assert!(a <= len, "{ctx}: jump {a} past the end at {i}");
                    jump_to_end += usize::from(a == len);
                }
                _ => {}
            }
        }
    }
    assert_eq!(jump_to_end, 1889);
    // Largest range in the shipped data; the reader limit (`MAX_RANGE_LEN`) is far above it.
    assert_eq!(max_range_len, 93);
    assert_eq!(
        store_on_list.keys().copied().collect::<Vec<_>>(),
        [523821, 524077, 555071, 555327]
    );
    assert_eq!(store_on_list.values().sum::<u32>(), 4);
    assert_eq!(
        load_on_ref.keys().copied().collect::<Vec<_>>(),
        [929760, 929761, 929764, 929793, 929838, 929869]
    );
    assert!(load_on_ref.values().all(|&n| n == 7), "{load_on_ref:?}");
    assert_eq!(callb, 106_115);
    assert_eq!(callb_ids.len(), 325);
    assert!(callb_ids.iter().all(|id| (1..=757).contains(id)));
    assert_eq!((callm.len(), spawn.len()), (25, 183));
    for id in callm.iter().chain(&spawn) {
        assert!(modules.contains_key(id), "target module {id} missing");
        assert!(vfs.exists(&module_path(*id)), "target module {id} missing");
    }
    assert!(modules[&52554].code.is_empty());

    let db = parse_db(&vfs.read(DB_PATH).unwrap()).unwrap();
    assert_eq!(db.version, 1);
    assert_eq!(db.entries.len(), 280);
    for e in &db.entries {
        let m = modules
            .get(&e.id)
            .unwrap_or_else(|| panic!("entry {} without a file", e.id));
        let argc = m.debug.as_ref().unwrap().argc;
        assert_eq!(e.params.len() as u32, argc, "{}", e.name);
    }
    let listed: BTreeSet<u32> = db.entries.iter().map(|e| e.id).collect();
    let unlisted: Vec<u32> = modules
        .keys()
        .copied()
        .filter(|id| !listed.contains(id))
        .collect();
    assert_eq!(
        unlisted,
        [
            523821, 524077, 555071, 555327, 590203, 590204, 590207, 590236, 590281, 590312, 815535
        ]
    );
}
