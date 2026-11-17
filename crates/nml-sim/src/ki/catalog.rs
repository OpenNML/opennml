//! Catalog of the AI prototypes: one loaded module per entry of `nml.db` (re/ki-vm 12.3 P14). Each instance (thread)
//! copies its code and initial variables from its prototype (re/ki-vm 4.1, 8; `docs/formats/kimodule.md`).

use std::collections::BTreeMap;

use anyhow::Context;
use nml_formats::kimodule::{DB_PATH, KiModule, module_path, parse, parse_db};
use nml_rda::Vfs;

use super::vars::VarTable;
use crate::rules::Fnv64;

/// A module of the catalog: immutable, template for the threads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KiPrototype {
    pub id: u32,
    /// Name of the entry in `nml.db`.
    pub name: String,
    /// Parameter count according to `nml.db`.
    pub params: u32,
    pub code: Vec<u32>,
    /// Initial state of the variables with `CONSTDAT`.
    pub vars: VarTable,
    /// FNV-1a 64 over the file bytes (re/ki-vm 12.3 P14).
    pub checksum: u64,
}

/// Prototypes by module ID.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KiCatalog {
    pub modules: BTreeMap<u32, KiPrototype>,
}

/// Builds the prototype for a module that has been read; `bytes` are the file bytes (for the checksum).
pub fn prototype(id: u32, name: &str, params: u32, module: &KiModule, bytes: &[u8]) -> KiPrototype {
    let mut hash = Fnv64::default();
    hash.write(bytes);
    KiPrototype {
        id,
        name: name.to_string(),
        params,
        code: module.code.clone(),
        vars: VarTable::from_module(&module.ranges, &module.inits),
        checksum: hash.0,
    }
}

impl KiCatalog {
    /// Reads `nml.db` and then, per entry in database order, the module file. `nml.db` and all
    /// module files (path and bytes) go into the checksum of the rules; a missing or
    /// corrupted file is an error with the path (re/ki-vm 12.3 P14).
    pub fn load(vfs: &Vfs, hash: &mut Fnv64) -> anyhow::Result<Self> {
        let db_bytes = vfs
            .read(DB_PATH)
            .with_context(|| format!("reading {DB_PATH}"))?;
        hash.write(DB_PATH.as_bytes());
        hash.write(&db_bytes);
        let db = parse_db(&db_bytes).with_context(|| format!("parsing {DB_PATH}"))?;
        let mut modules = BTreeMap::new();
        for entry in &db.entries {
            let path = module_path(entry.id);
            let bytes = vfs.read(&path).with_context(|| format!("reading {path}"))?;
            hash.write(path.as_bytes());
            hash.write(&bytes);
            let module = parse(&bytes).with_context(|| format!("parsing {path}"))?;
            let proto = prototype(
                entry.id,
                &entry.name,
                entry.params.len() as u32,
                &module,
                &bytes,
            );
            modules.insert(entry.id, proto);
        }
        Ok(Self { modules })
    }

    pub fn get(&self, id: u32) -> Option<&KiPrototype> {
        self.modules.get(&id)
    }

    /// Catalog from finished prototypes (tests); for a duplicate ID the last one wins.
    pub fn from_prototypes(protos: impl IntoIterator<Item = KiPrototype>) -> Self {
        Self {
            modules: protos.into_iter().map(|p| (p.id, p)).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ki::testkit::{self, int, word};

    fn module() -> KiModule {
        KiModule {
            blocks: Vec::new(),
            header: None,
            ranges: vec![(0, 3)],
            inits: vec![(int(1), 7)],
            code: vec![word(1, int(0)), word(0, int(1))],
            names: Vec::new(),
            debug: None,
        }
    }

    #[test]
    fn prototype_copies_code_and_initial_values() {
        let m = module();
        let p = prototype(42, "demo", 2, &m, b"abc");
        assert_eq!((p.id, p.name.as_str(), p.params), (42, "demo", 2));
        assert_eq!(p.code, m.code);
        assert_eq!(p.vars.get(int(0)), 0);
        assert_eq!(p.vars.get(int(1)), 7);
        assert_eq!(p.vars.get(int(2)), 0);
        let mut fnv = Fnv64::default();
        fnv.write(b"abc");
        assert_eq!(p.checksum, fnv.0);
        // Different bytes, different checksum.
        assert_ne!(prototype(42, "demo", 2, &m, b"abd").checksum, p.checksum);
    }

    #[test]
    fn testkit_builds_catalog_by_id() {
        let a = testkit::proto(1, &[(0, 2)], &[(int(0), 5)], &[word(0, int(0))]);
        let b = testkit::proto(2, &[], &[], &[]);
        let catalog = testkit::catalog(vec![a.clone(), b]);
        assert_eq!(catalog.modules.len(), 2);
        assert_eq!(catalog.get(1), Some(&a));
        assert_eq!(a.name, "test1");
        assert_eq!(a.vars.get(int(0)), 5);
        assert!(catalog.get(3).is_none());
        assert!(KiCatalog::default().modules.is_empty());
    }
}
