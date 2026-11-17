//! AI modules (`.mod`) and module database (`nml.db`), plus a disassembler.
//!
//! Format: `docs/formats/kimodule.md`. The reader follows the loader `FUN_004b50d0` of the original;
//! Deviation (E-15, A7): lengths are checked instead of writing into fixed fields.

use std::collections::BTreeMap;

use crate::FormatError;
use crate::binary::Reader;
use crate::text::decode_cp1252;

/// Folder of the module files in the game installation.
pub const MODULE_DIR: &str = "data/kimodules";
/// Path of the module database.
pub const DB_PATH: &str = "data/kimodules/nml.db";
/// Names of the opcodes 0-9.
pub const OP_NAMES: [&str; 10] = [
    "LOAD", "STORE", "PUSHA", "STORE3", "CALLB", "CALLM", "SPAWN", "JMP", "JZ", "JNZ",
];
/// Operand of `STORE`/`STORE3`: discard the value.
pub const DROP: u32 = 0x0FFF_FFFF;

/// Longest variable name in `VARMAP` (the original has a 64-byte field with a null byte).
const MAX_NAME: usize = 63;
/// Longest source path in `DEBUGGER` (256-byte field with a null byte).
const MAX_SOURCE: usize = 255;

/// Path of the module file for a module ID.
pub fn module_path(id: u32) -> String {
    format!("{MODULE_DIR}/{id:08}.mod")
}

/// A loaded AI module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KiModule {
    /// Identifiers of the blocks read in file order (without padding characters).
    pub blocks: Vec<String>,
    pub header: Option<u32>,
    /// `VARIABLE`: (range ID, length).
    pub ranges: Vec<(u32, u32)>,
    /// `CONSTDAT`: (address, value).
    pub inits: Vec<(u32, i32)>,
    /// `CODEDATA`: words of opcode (bits 28-31) and operand (bits 0-27).
    pub code: Vec<u32>,
    /// `VARMAP`: (name, address).
    pub names: Vec<(String, u32)>,
    pub debug: Option<KiDebug>,
}

/// Contents of the `DEBUGGER` block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KiDebug {
    pub source: String,
    pub argc: u32,
    pub signature: String,
    pub lines: Vec<u32>,
}

/// Remaining length after the read cursor.
fn left(data: &[u8], r: &Reader) -> usize {
    data.len().saturating_sub(r.pos())
}

/// Count for elements of `item` bytes, limited by the remaining length after the count.
fn items(data: &[u8], r: &mut Reader, item: usize, what: &str) -> Result<usize, FormatError> {
    let max = left(data, r).saturating_sub(4) / item;
    r.count(max, what)
}

/// Length and text; the length is at most `max`, reading past the end is `Truncated`.
fn text(r: &mut Reader, max: usize, what: &str) -> Result<String, FormatError> {
    let len = r.count(max, &format!("{what} (length, at most {max})"))?;
    Ok(decode_cp1252(r.take(len, what)?))
}

/// Length and text without a fixed upper limit (only the remaining length limits).
fn text_any(data: &[u8], r: &mut Reader, what: &str) -> Result<String, FormatError> {
    let max = left(data, r);
    text(r, max, what)
}

/// Largest length of a variable range: the index of an address has 16 bits, more elements cannot
/// be addressed. The limit protects against memory explosion with corrupted modules; the original
/// does not check it (`FUN_00413680` allocates the fields unchecked).
pub const MAX_RANGE_LEN: u32 = 0x1_0000;

fn read_ranges(data: &[u8], r: &mut Reader) -> Result<Vec<(u32, u32)>, FormatError> {
    let n = items(data, r, 8, "AI module VARIABLE: count")?;
    (0..n)
        .map(|_| {
            let id = r.u32("AI module VARIABLE: range ID")?;
            let len = r.u32("AI module VARIABLE: length")?;
            if len > MAX_RANGE_LEN {
                return Err(FormatError::Invalid(format!(
                    "AI module VARIABLE: length {len} of range {id:#x} greater than {MAX_RANGE_LEN}"
                )));
            }
            Ok((id, len))
        })
        .collect()
}

fn read_inits(data: &[u8], r: &mut Reader) -> Result<Vec<(u32, i32)>, FormatError> {
    let n = items(data, r, 8, "AI module CONSTDAT: count")?;
    (0..n)
        .map(|_| {
            Ok((
                r.u32("AI module CONSTDAT: address")?,
                r.i32("AI module CONSTDAT: value")?,
            ))
        })
        .collect()
}

fn read_names(data: &[u8], r: &mut Reader) -> Result<Vec<(String, u32)>, FormatError> {
    // At least length (4) and address (4) per entry.
    let n = items(data, r, 8, "AI module VARMAP: count")?;
    (0..n)
        .map(|_| {
            let name = text(r, MAX_NAME, "AI module VARMAP: variable name")?;
            Ok((name, r.u32("AI module VARMAP: address")?))
        })
        .collect()
}

fn read_debug(data: &[u8], r: &mut Reader) -> Result<KiDebug, FormatError> {
    let source = text(r, MAX_SOURCE, "AI module DEBUGGER: source path")?;
    let argc = r.u32("AI module DEBUGGER: parameter count")?;
    let signature = text_any(data, r, "AI module DEBUGGER: signature")?;
    let n = items(data, r, 4, "AI module DEBUGGER: line count")?;
    let lines = (0..n)
        .map(|_| r.u32("AI module DEBUGGER: code index"))
        .collect::<Result<_, _>>()?;
    Ok(KiDebug {
        source,
        argc,
        signature,
        lines,
    })
}

/// Reads an AI module like the loader of the original.
pub fn parse(data: &[u8]) -> Result<KiModule, FormatError> {
    let mut r = Reader::new(data);
    let mut blocks = Vec::new();
    let mut header = None;
    let mut ranges = None;
    let mut inits = None;
    let mut code = None;
    let mut names = Vec::new();
    let mut debug = None;
    // A remainder under 12 bytes ends the reading without an error.
    while left(data, &r) >= 12 {
        let tag = r.take(8, "AI module: block identifier")?;
        let size = r.u32("AI module: block size")? as usize;
        let end = tag
            .iter()
            .rposition(|&b| b != b' ' && b != 0)
            .map_or(0, |p| p + 1);
        blocks.push(decode_cp1252(&tag[..end]));
        if tag.starts_with(b"KIMODULE") {
            // Wrapper: its size is not used.
        } else if tag.starts_with(b"HEADER") {
            header = Some(r.u32("AI module HEADER: version")?);
        } else if tag.starts_with(b"VARIABLE") {
            ranges = Some(read_ranges(data, &mut r)?);
        } else if tag.starts_with(b"CONSTDAT") {
            inits = Some(read_inits(data, &mut r)?);
        } else if tag.starts_with(b"CODEDATA") {
            // `size >> 2` words are read; a remainder of the size stays unread.
            let words = r.take((size >> 2) * 4, "AI module CODEDATA: code")?;
            code = Some(
                words
                    .chunks_exact(4)
                    .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
                    .collect(),
            );
        } else if tag.starts_with(b"VARMAP") {
            names = read_names(data, &mut r)?;
        } else if tag.starts_with(b"DEBUGGER") {
            debug = Some(read_debug(data, &mut r)?);
            break;
        } else {
            r.skip(size, "AI module: unknown block")?;
        }
    }
    let missing = |name: &str| FormatError::Invalid(format!("AI module without block {name}"));
    Ok(KiModule {
        blocks,
        header,
        ranges: ranges.ok_or_else(|| missing("VARIABLE"))?,
        inits: inits.ok_or_else(|| missing("CONSTDAT"))?,
        code: code.ok_or_else(|| missing("CODEDATA"))?,
        names,
        debug,
    })
}

/// The module database `nml.db`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleDb {
    pub version: u32,
    pub root: String,
    pub entries: Vec<DbEntry>,
}

/// An entry of the module database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbEntry {
    pub name: String,
    pub source: String,
    /// Parameter types (0 integer, 1 boolean, 2 building, 3 unit, 4 object).
    pub params: Vec<u32>,
    /// Return type, -1 none.
    pub ret: i32,
    pub id: u32,
}

/// Reads `nml.db` (identifier `MODB`).
pub fn parse_db(data: &[u8]) -> Result<ModuleDb, FormatError> {
    let mut r = Reader::new(data);
    if r.take(4, "AI module database: identifier")? != b"MODB" {
        return Err(FormatError::Invalid(
            "AI module database without identifier MODB".to_string(),
        ));
    }
    let version = r.u32("AI module database: version")?;
    let root = text_any(data, &mut r, "AI module database: root")?;
    // At least five fields of 4 bytes each per entry.
    let n = items(data, &mut r, 20, "AI module database: count")?;
    let mut entries = Vec::with_capacity(n);
    for _ in 0..n {
        let name = text_any(data, &mut r, "AI module database: name")?;
        let source = text_any(data, &mut r, "AI module database: source")?;
        let count = items(data, &mut r, 4, "AI module database: parameter count")?;
        let params = (0..count)
            .map(|_| r.u32("AI module database: parameter type"))
            .collect::<Result<_, _>>()?;
        entries.push(DbEntry {
            name,
            source,
            params,
            ret: r.i32("AI module database: return type")?,
            id: r.u32("AI module database: module ID")?,
        });
    }
    Ok(ModuleDb {
        version,
        root,
        entries,
    })
}

/// One line per code word (disassembly listing).
pub fn disassemble(m: &KiModule) -> Vec<String> {
    let names: BTreeMap<u32, &str> = m
        .names
        .iter()
        .map(|(name, addr)| (addr & 0x0FFF_FFFF, name.as_str()))
        .collect();
    m.code
        .iter()
        .enumerate()
        .map(|(i, &w)| {
            let (op, a) = ((w >> 28) as usize, w & 0x0FFF_FFFF);
            let name = OP_NAMES.get(op).copied().unwrap_or("-");
            let arg = match op {
                0 | 1 | 3 if a == DROP => "drop".to_string(),
                0 | 1 | 3 => names.get(&a).map_or_else(
                    || format!("k{} t{} #{}", a >> 24, (a >> 16) & 0xFF, a & 0xFFFF),
                    |n| (*n).to_string(),
                ),
                _ => a.to_string(),
            };
            format!("{i:5} {w:08x} {name:6} {arg}")
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper for writing synthetic module files.
    #[derive(Default)]
    struct W(Vec<u8>);

    impl W {
        fn u32(&mut self, v: u32) -> &mut Self {
            self.0.extend_from_slice(&v.to_le_bytes());
            self
        }
        fn tag(&mut self, name: &str, size: u32) -> &mut Self {
            let mut t = name.as_bytes().to_vec();
            t.resize(8, b' ');
            self.0.extend_from_slice(&t);
            self.u32(size)
        }
        fn str(&mut self, s: &str) -> &mut Self {
            self.u32(s.len() as u32);
            self.0.extend_from_slice(s.as_bytes());
            self
        }
    }

    fn header(w: &mut W) {
        w.tag("KIMODULE", 0).tag("HEADER", 4).u32(1);
        w.tag("VARIABLE", 12).u32(1).u32(0).u32(3);
        w.tag("CONSTDAT", 12)
            .u32(1)
            .u32(0x0000_0001)
            .u32(-5i32 as u32);
    }

    fn full_module() -> Vec<u8> {
        let mut w = W::default();
        header(&mut w);
        w.tag("CODEDATA", 4).u32(0x4000_0001);
        w.tag("VARMAP", 0).u32(1).str("x").u32(0);
        w.tag("DEBUGGER", 0)
            .str("D:\\x\\a.aml")
            .u32(2)
            .str("a( 0, 0 ) -1")
            .u32(2)
            .u32(0)
            .u32(5);
        w.0
    }

    #[test]
    fn parses_the_seven_blocks_in_file_order() {
        let m = parse(&full_module()).unwrap();
        assert_eq!(
            m.blocks,
            [
                "KIMODULE", "HEADER", "VARIABLE", "CONSTDAT", "CODEDATA", "VARMAP", "DEBUGGER"
            ]
        );
        assert_eq!(m.header, Some(1));
        assert_eq!(m.ranges, [(0x0000_0000, 3)]);
        assert_eq!(m.inits, [(1, -5)]);
        assert_eq!(m.code, [0x4000_0001]);
        assert_eq!(m.names, [("x".to_string(), 0)]);
        assert_eq!(
            m.debug,
            Some(KiDebug {
                source: "D:\\x\\a.aml".to_string(),
                argc: 2,
                signature: "a( 0, 0 ) -1".to_string(),
                lines: vec![0, 5],
            })
        );
    }

    #[test]
    fn codedata_reads_size_div_4_and_continues_behind_the_words() {
        let mut w = W::default();
        header(&mut w);
        // Size 6: one word is read, reading continues right after that word (not at the end of the declared size).
        w.tag("CODEDATA", 6).u32(0x7000_0000);
        w.tag("VARMAP", 0).u32(1).str("y").u32(7);
        let m = parse(&w.0).unwrap();
        assert_eq!(m.code, [0x7000_0000]);
        assert_eq!(m.names, [("y".to_string(), 7)]);
        assert_eq!(m.debug, None);
        // A remainder under 12 bytes ends the reading without an error.
        let mut w = W::default();
        header(&mut w);
        w.tag("CODEDATA", 0).0.extend_from_slice(&[1, 2, 3]);
        assert_eq!(parse(&w.0).unwrap().code, Vec::<u32>::new());
    }

    #[test]
    fn unknown_blocks_are_skipped_by_their_size() {
        let mut w = W::default();
        header(&mut w);
        w.tag("UNKNOWN", 5);
        w.0.extend_from_slice(&[9; 5]);
        w.tag("CODEDATA", 4).u32(1);
        let m = parse(&w.0).unwrap();
        assert_eq!(m.code, [1]);
        assert_eq!(m.blocks[4], "UNKNOWN");
        // Check the length: the block extends past the end of the file.
        let mut w = W::default();
        header(&mut w);
        w.tag("UNKNOWN", 100).u32(0);
        assert!(matches!(parse(&w.0), Err(FormatError::Truncated { .. })));
    }

    #[test]
    fn missing_code_block_is_an_error() {
        let mut w = W::default();
        header(&mut w);
        let err = parse(&w.0).unwrap_err().to_string();
        assert!(err.contains("AI module without block CODEDATA"), "{err}");
        let err = parse(&[]).unwrap_err().to_string();
        assert!(err.contains("AI module without block VARIABLE"), "{err}");
    }

    #[test]
    fn truncated_and_oversized_inputs_fail_in_german() {
        let full = full_module();
        // Every abort is a result, never a panic.
        for cut in 0..full.len() {
            let _ = parse(&full[..cut]);
        }
        // Header cut off in VARIABLE.
        let mut w = W::default();
        w.tag("KIMODULE", 0)
            .tag("VARIABLE", 12)
            .0
            .extend_from_slice(&[1, 0]);
        let err = parse(&w.0).unwrap_err().to_string();
        assert!(err.contains("AI module VARIABLE"), "{err}");
        // Huge count.
        let mut w = W::default();
        w.tag("VARIABLE", 12).u32(0x7FFF_FFFF).u32(0).u32(0);
        let err = parse(&w.0).unwrap_err().to_string();
        assert!(err.contains("AI module VARIABLE: count"), "{err}");
        // Range length: 0x1_0000 is valid, 0x1_0001 is not.
        for (len, ok) in [
            (MAX_RANGE_LEN, true),
            (MAX_RANGE_LEN + 1, false),
            (u32::MAX, false),
        ] {
            let mut w = W::default();
            w.tag("VARIABLE", 12).u32(1).u32(0).u32(len);
            let res = parse(&w.0);
            // Without the remaining blocks a valid range ends in "without block", never in the length.
            let err = res.unwrap_err().to_string();
            assert_eq!(
                !err.contains("AI module VARIABLE: length"),
                ok,
                "length {len}: {err}"
            );
            if !ok {
                assert!(err.contains("AI module"), "{err}");
            }
        }
        // Name with 64 bytes (63 are allowed).
        for (len, ok) in [(63, true), (64, false)] {
            let mut w = W::default();
            header(&mut w);
            w.tag("CODEDATA", 0);
            w.tag("VARMAP", 0).u32(1).str(&"n".repeat(len)).u32(0);
            let res = parse(&w.0);
            assert_eq!(res.is_ok(), ok, "name with {len} bytes");
            if !ok {
                let err = res.unwrap_err().to_string();
                assert!(err.contains("AI module VARMAP"), "{err}");
            }
        }
        // Path with 256 bytes (255 are allowed).
        for (len, ok) in [(255, true), (256, false)] {
            let mut w = W::default();
            header(&mut w);
            w.tag("CODEDATA", 0);
            w.tag("DEBUGGER", 0)
                .str(&"p".repeat(len))
                .u32(0)
                .str("")
                .u32(0);
            let res = parse(&w.0);
            assert_eq!(res.is_ok(), ok, "path with {len} bytes");
            if !ok {
                let err = res.unwrap_err().to_string();
                assert!(err.contains("AI module DEBUGGER"), "{err}");
            }
        }
    }

    #[test]
    fn nml_db_round_trip() {
        let mut w = W::default();
        w.0.extend_from_slice(b"MODB");
        w.u32(1).str("D:\\NML\\Data\\KIModules").u32(2);
        w.str("alpha")
            .str("a.aml")
            .u32(2)
            .u32(0)
            .u32(3)
            .u32(-1i32 as u32)
            .u32(100);
        w.str("beta").str("b.aml").u32(0).u32(2).u32(101);
        let db = parse_db(&w.0).unwrap();
        assert_eq!(db.version, 1);
        assert_eq!(db.root, "D:\\NML\\Data\\KIModules");
        assert_eq!(
            db.entries,
            [
                DbEntry {
                    name: "alpha".to_string(),
                    source: "a.aml".to_string(),
                    params: vec![0, 3],
                    ret: -1,
                    id: 100
                },
                DbEntry {
                    name: "beta".to_string(),
                    source: "b.aml".to_string(),
                    params: vec![],
                    ret: 2,
                    id: 101
                },
            ]
        );
        assert!(parse_db(b"XXXX\x01\0\0\0").is_err());
        for cut in 0..w.0.len() {
            assert!(parse_db(&w.0[..cut]).is_err(), "abort at {cut}");
        }
    }

    #[test]
    fn disassembler_names_variables_and_drop() {
        let m = KiModule {
            blocks: vec![],
            header: Some(1),
            ranges: vec![],
            inits: vec![],
            code: vec![
                0x0000_0000,
                0x1000_0000 | DROP,
                0x2100_0004,
                0x3002_0001,
                0x4000_0123,
                0x7000_0009,
                0xA000_0001,
            ],
            names: vec![("x".to_string(), 0)],
            debug: None,
        };
        assert_eq!(
            disassemble(&m),
            [
                "    0 00000000 LOAD   x",
                "    1 1fffffff STORE  drop",
                "    2 21000004 PUSHA  16777220",
                "    3 30020001 STORE3 k0 t2 #1",
                "    4 40000123 CALLB  291",
                "    5 70000009 JMP    9",
                "    6 a0000001 -      1",
            ]
        );
        assert_eq!(module_path(52554), "data/kimodules/00052554.mod");
    }
}
