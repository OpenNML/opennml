//! Desync report: before adopting the host state, the deviating client writes a folder
//! `netlog/desync_<YYYY_MM_DD_hhmmss>_<n>/` with five files:
//!
//! - `host.nmlsave`, `local.nmlsave`: both simulation parts as [`nml_sim::save::SaveGame::to_bytes`] (loadable,
//!   commands empty);
//! - `host.nmllog`, `local.nmllog`: both network states in full ([`NetState::to_bytes`], with command log);
//! - `report.txt`: turn, tick, both checksums, both values for each state part ([`part_checksums`], deviating ones
//!   preceded by `≠`), slots and names, the commands of the last three bundles.
//!
//! The original has no report: the error path of the checksum is never reached (E-260, E-261).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use chrono::{Datelike, Local, Timelike};
use nml_net::{LocalTime, Seat, TurnBundle};
use nml_sim::save::{checksum, part_checksums};

use super::NetState;

/// Local time of this machine (`localtime`; model: `save_files.rs`).
pub fn local_now() -> LocalTime {
    let t = Local::now();
    LocalTime {
        year: u16::try_from(t.year()).unwrap_or(0),
        month: t.month() as u8,
        day: t.day() as u8,
        hour: t.hour() as u8,
        minute: t.minute() as u8,
        second: t.second() as u8,
    }
}

/// Name of the report folder: `desync_<YYYY_MM_DD_hhmmss>_<n>`.
pub fn report_dir_name(when: LocalTime, n: u32) -> String {
    format!(
        "desync_{:04}_{:02}_{:02}_{:02}{:02}{:02}_{n}",
        when.year, when.month, when.day, when.hour, when.minute, when.second
    )
}

/// Content of `report.txt`. `turn` is the turn of the bundle with the sync, `step` the tick of both states
/// (`e(turn) + 1`), `bundles` the last (at most three) executed bundles, oldest first.
#[allow(clippy::too_many_arguments)]
pub fn report_text(
    n: u32,
    turn: u32,
    step: u64,
    local: &NetState,
    host: &NetState,
    seats: &[Seat; 8],
    bundles: &[TurnBundle],
    when: LocalTime,
) -> String {
    let mut out = String::new();
    // `write!` into a `String` does not fail.
    let _ = writeln!(out, "Desync report {n} (No Man's Land, network match)");
    let _ = writeln!(
        out,
        "Local time: {:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        when.year, when.month, when.day, when.hour, when.minute, when.second
    );
    let _ = writeln!(out, "Turn: {turn}");
    let _ = writeln!(
        out,
        "Tick: {step} (state after execution tick {})",
        step.saturating_sub(1)
    );
    let _ = writeln!(out, "Checksum host: {:#018x}", checksum(&host.save.state));
    let _ = writeln!(out, "Checksum local: {:#018x}", checksum(&local.save.state));
    let _ = writeln!(
        out,
        "Command log: host {} commands, local {} commands{}",
        host.log.len(),
        local.log.len(),
        if host.log == local.log {
            ", equal"
        } else {
            ", different"
        }
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "Slots:");
    for (slot, seat) in seats.iter().enumerate() {
        let machine = match seat.peer {
            Some(0) => "Machine 0 (host)".to_string(),
            Some(p) => format!("Machine {p}"),
            None => "no machine".to_string(),
        };
        let _ = writeln!(out, "  Slot {slot}: {machine}, \"{}\"", seat.name);
    }
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "Checksums per state part (host, local; ≠ = deviating):"
    );
    let host_parts = part_checksums(&host.save.state);
    let local_parts = part_checksums(&local.save.state);
    for ((name, h), (_, l)) in host_parts.iter().zip(&local_parts) {
        let mark = if h == l { ' ' } else { '≠' };
        let _ = writeln!(out, "{mark} Part {name}: host {h:#018x}, local {l:#018x}");
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "Commands of the last {} bundles:", bundles.len());
    for b in bundles {
        let _ = writeln!(out, "  Bundle {}:", b.turn);
        if b.commands.is_empty() {
            let _ = writeln!(out, "    no commands");
        }
        for c in &b.commands {
            let _ = writeln!(out, "    Slot {}: {:?}", c.player, c.kind);
        }
        for c in &b.control {
            let _ = writeln!(out, "    Control entry: {c:?}");
        }
    }
    out
}

/// Writes report `n` to `dir/desync_<YYYY_MM_DD_hhmmss>_<n>/` and returns the folder. Parameters as for
/// [`report_text`]; `dir` is `NetPaths::netlog`.
#[allow(clippy::too_many_arguments)]
pub fn write_desync(
    dir: &Path,
    n: u32,
    turn: u32,
    step: u64,
    local: &NetState,
    host: &NetState,
    seats: &[Seat; 8],
    bundles: &[TurnBundle],
    when: LocalTime,
) -> std::io::Result<PathBuf> {
    let out = dir.join(report_dir_name(when, n));
    std::fs::create_dir_all(&out)?;
    std::fs::write(out.join("host.nmlsave"), host.save.to_bytes())?;
    std::fs::write(out.join("local.nmlsave"), local.save.to_bytes())?;
    std::fs::write(out.join("host.nmllog"), host.to_bytes())?;
    std::fs::write(out.join("local.nmllog"), local.to_bytes())?;
    std::fs::write(
        out.join("report.txt"),
        report_text(n, turn, step, local, host, seats, bundles, when),
    )?;
    Ok(out)
}
