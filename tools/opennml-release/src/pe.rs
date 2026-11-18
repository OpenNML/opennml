//! Minimal PE32+ reader: machine, subsystem and imported DLL names. Every offset is checked
//! against the input; a broken or foreign file is an error, never a panic.

use anyhow::{Context, anyhow, bail};

pub struct PeInfo {
    pub machine: u16,
    pub subsystem: u16,
    pub imports: Vec<String>,
}

const PE32_PLUS_MAGIC: u16 = 0x20b;
const SECTION_SIZE: usize = 40;
const IMPORT_DESCRIPTOR_SIZE: usize = 20;
const MAX_IMPORTS: usize = 4096;

fn u16_at(bytes: &[u8], at: usize) -> anyhow::Result<u16> {
    let end = at.checked_add(2).context("offset overflow")?;
    let b = bytes
        .get(at..end)
        .ok_or_else(|| anyhow!("file ends at offset {at:#x}"))?;
    Ok(u16::from_le_bytes([b[0], b[1]]))
}

fn u32_at(bytes: &[u8], at: usize) -> anyhow::Result<u32> {
    let end = at.checked_add(4).context("offset overflow")?;
    let b = bytes
        .get(at..end)
        .ok_or_else(|| anyhow!("file ends at offset {at:#x}"))?;
    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn add(a: usize, b: usize) -> anyhow::Result<usize> {
    a.checked_add(b).context("offset overflow")
}

struct Section {
    rva: u32,
    /// Bytes of the section that are in the file.
    raw_size: u32,
    raw: u32,
}

fn rva_to_offset(sections: &[Section], rva: u32) -> anyhow::Result<usize> {
    for s in sections {
        if let Some(delta) = rva.checked_sub(s.rva)
            && delta < s.raw_size
        {
            let offset = s
                .raw
                .checked_add(delta)
                .context("section offset overflow")?;
            return Ok(offset as usize);
        }
    }
    bail!("RVA {rva:#x} is in no section")
}

fn c_string_at(bytes: &[u8], at: usize) -> anyhow::Result<String> {
    let tail = bytes
        .get(at..)
        .ok_or_else(|| anyhow!("string offset {at:#x} is outside the file"))?;
    let len = tail
        .iter()
        .take(512)
        .position(|&b| b == 0)
        .ok_or_else(|| anyhow!("unterminated string at offset {at:#x}"))?;
    Ok(String::from_utf8_lossy(&tail[..len]).into_owned())
}

pub fn read_pe(bytes: &[u8]) -> anyhow::Result<PeInfo> {
    if bytes.get(..2) != Some(b"MZ") {
        bail!("not a PE file: missing MZ header");
    }
    let pe = u32_at(bytes, 0x3c)? as usize;
    if bytes.get(pe..add(pe, 4)?) != Some(b"PE\0\0") {
        bail!("not a PE file: missing PE signature");
    }
    let coff = add(pe, 4)?;
    let machine = u16_at(bytes, add(coff, 0)?)?;
    let section_count = u16_at(bytes, add(coff, 2)?)? as usize;
    let opt_size = u16_at(bytes, add(coff, 16)?)? as usize;
    let opt = add(coff, 20)?;
    let magic = u16_at(bytes, opt)?;
    if magic != PE32_PLUS_MAGIC {
        bail!("not a 64-bit PE file (optional header magic {magic:#x}, machine {machine:#x})");
    }
    // Subsystem at +68; data directory count at +108; directories from +112.
    let subsystem = u16_at(bytes, add(opt, 68)?)?;
    let dir_count = u32_at(bytes, add(opt, 108)?)? as usize;

    let sections_at = add(opt, opt_size)?;
    let mut sections = Vec::new();
    for i in 0..section_count {
        let at = add(sections_at, i * SECTION_SIZE)?;
        let raw_size = u32_at(bytes, add(at, 16)?)?;
        sections.push(Section {
            rva: u32_at(bytes, add(at, 12)?)?,
            raw_size,
            raw: u32_at(bytes, add(at, 20)?)?,
        });
    }

    let mut imports = Vec::new();
    // Data directory 1 is the import table.
    if dir_count > 1 && opt_size >= 112 + 16 {
        let import_rva = u32_at(bytes, add(opt, 112 + 8)?)?;
        if import_rva != 0 {
            let table = rva_to_offset(&sections, import_rva)?;
            for i in 0..MAX_IMPORTS {
                let at = add(table, i * IMPORT_DESCRIPTOR_SIZE)?;
                let descriptor = bytes
                    .get(at..add(at, IMPORT_DESCRIPTOR_SIZE)?)
                    .ok_or_else(|| anyhow!("import table is outside the file"))?;
                if descriptor.iter().all(|&b| b == 0) {
                    return Ok(PeInfo {
                        machine,
                        subsystem,
                        imports,
                    });
                }
                let name_rva = u32_at(bytes, add(at, 12)?)?;
                let name = rva_to_offset(&sections, name_rva)?;
                imports.push(c_string_at(bytes, name)?);
            }
            bail!("import table has no terminator within {MAX_IMPORTS} entries");
        }
    }
    Ok(PeInfo {
        machine,
        subsystem,
        imports,
    })
}

#[cfg(test)]
mod tests {
    use std::{env, fs};

    use super::*;

    fn own_binary() -> Vec<u8> {
        fs::read(env::current_exe().unwrap()).unwrap()
    }

    #[test]
    fn read_pe_reads_this_test_binary() {
        let info = read_pe(&own_binary()).unwrap();
        assert_eq!(info.machine, 0x8664);
        assert_eq!(info.subsystem, 3);
        assert!(
            info.imports
                .iter()
                .any(|i| i.eq_ignore_ascii_case("kernel32.dll"))
        );
    }

    #[test]
    fn rva_to_offset_maps_only_data_that_is_in_the_file() {
        let sections = [Section {
            rva: 0x1000,
            raw_size: 0x200,
            raw: 0x400,
        }];
        assert_eq!(rva_to_offset(&sections, 0x1000).unwrap(), 0x400);
        assert_eq!(rva_to_offset(&sections, 0x11ff).unwrap(), 0x5ff);
        // Past the raw data (virtual padding): no file offset.
        assert!(rva_to_offset(&sections, 0x1200).is_err());
        assert!(rva_to_offset(&sections, 0xfff).is_err());
    }

    #[test]
    fn read_pe_rejects_truncated_input() {
        let bytes = own_binary();
        for len in [0, 2, 64, 512, 4096] {
            assert!(read_pe(&bytes[..len]).is_err(), "{len} bytes");
        }
        assert!(read_pe(&[0u8; 4096]).is_err());

        // A 32-bit exe: machine 0x14c and PE32 magic.
        let pe = u32::from_le_bytes(bytes[0x3c..0x40].try_into().unwrap()) as usize;
        let mut x86 = bytes.clone();
        x86[pe + 4..pe + 6].copy_from_slice(&0x14cu16.to_le_bytes());
        x86[pe + 24..pe + 26].copy_from_slice(&0x10bu16.to_le_bytes());
        assert!(read_pe(&x86).is_err());

        // Import table RVA outside every section.
        let mut bad = bytes.clone();
        bad[pe + 24 + 112 + 8..pe + 24 + 112 + 12].copy_from_slice(&0xffff_fff0u32.to_le_bytes());
        assert!(read_pe(&bad).is_err());

        // Every prefix of the first 2 KiB must not panic.
        for len in 0..bytes.len().min(2048) {
            let _ = read_pe(&bytes[..len]);
        }
    }
}
