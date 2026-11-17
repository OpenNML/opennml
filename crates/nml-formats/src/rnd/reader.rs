//! Line reader with preprocessor of the map scripts (`FUN_005424B0` `0x005424B0`, see re/random-map Z.5.3)
//! including file access `FUN_00546930` (`0x00546930`).
//!
//! The reader is the line supplier of the dispatcher (`FUN_00545C10`): every line, also inside blocks, runs
//! through [`ScriptReader::next_line`]. The state (block comment, "active", "branch chosen", random branch,
//! roll, sum) belongs to the generator, not to the file; an `#include` only opens another level and restores
//! nothing after returning (D-343).

use crate::FormatError;
use crate::config::trim_left;
use crate::rnd::scan::{scan_i, scan_word};
use crate::text::decode_cp1252;

/// Greatest depth of open files (main script = 1). The original has no limit (D-344, stack overflow on
/// cycles); here [`ScriptReader::open`] rejects deeper includes.
const MAX_DEPTH: usize = 16;

/// Path of an `#include` (`FUN_00542350`): directory of the **main script** (up to and including the last `/`,
/// only without `/` the last `\`) plus name. Without a separator the result is `""`; opening then fails and the
/// include is silently missing.
pub fn include_path(main: &str, name: &str) -> String {
    match main.rfind('/').or_else(|| main.rfind('\\')) {
        Some(slash) => format!("{}{name}", &main[..=slash]),
        None => String::new(),
    }
}

/// An open file: its lines (read like `GameFile_ReadLine`) and the reading position.
struct Level {
    path: String,
    lines: Vec<String>,
    next: usize,
}

impl Level {
    /// `GameFile_ReadLine` (`0x00574C00`): up to `\n`, every `\r` discarded, every other byte kept; a last
    /// line without `\n` counts, an empty remainder after the last `\n` does not. All following steps are C
    /// strings and see only the text up to the first `\0`.
    fn new(path: &str, bytes: &[u8]) -> Self {
        let bytes: Vec<u8> = bytes.iter().copied().filter(|&b| b != b'\r').collect();
        let mut lines: Vec<&[u8]> = bytes.split(|&b| b == b'\n').collect();
        if lines.last().is_some_and(|line| line.is_empty()) {
            lines.pop();
        }
        let lines = lines
            .into_iter()
            .map(|line| {
                let end = line.iter().position(|&b| b == 0).unwrap_or(line.len());
                decode_cp1252(&line[..end])
            })
            .collect();
        Level {
            path: path.to_string(),
            lines,
            next: 0,
        }
    }

    fn read(&mut self) -> Option<String> {
        let line = self.lines.get(self.next)?.clone();
        self.next += 1;
        Some(line)
    }
}

/// Flags of the generator (`G+0x108 ... +0x10B`, `+0x230`, `+0x234`).
#[derive(Default)]
struct Flags {
    /// `+0x108`: block comment open.
    comment: bool,
    /// `+0x109`: lines are active (otherwise they become empty).
    active: bool,
    /// `+0x10A`: a branch has already been chosen. Not preset in the original (heap leftover, D-326),
    /// here 0; `open` does not touch the flag (D-343).
    chosen: bool,
    /// `+0x10B`: lines of a `#chance` branch that was not hit become empty.
    random_off: bool,
    /// `+0x230`: roll of `#random` (101 after a hit). Not preset (D-326), here 0.
    roll: u32,
    /// `+0x234`: sum of the `#chance` values. Not preset (D-326), here 0.
    sum: u32,
}

/// Line reader including preprocessor. File access and randomness come from the host: `read` returns the
/// bytes of a path (`None` = missing), `random` the raw value of `rand()` (the CRT sequence of the
/// generator); the reader computes `% 100` itself.
pub struct ScriptReader<'a> {
    read: &'a mut dyn FnMut(&str) -> Option<Vec<u8>>,
    random: &'a mut dyn FnMut() -> i32,
    defines: Vec<String>,
    consts: Vec<(String, String)>,
    main_path: String,
    levels: Vec<Level>,
    flags: Flags,
}

impl<'a> ScriptReader<'a> {
    pub fn new(
        read: &'a mut dyn FnMut(&str) -> Option<Vec<u8>>,
        random: &'a mut dyn FnMut() -> i32,
        defines: Vec<String>,
    ) -> Self {
        ScriptReader {
            read,
            random,
            defines,
            consts: Vec::new(),
            main_path: String::new(),
            levels: Vec::new(),
            flags: Flags::default(),
        }
    }

    /// `FUN_00546930` `0x00546930`: opens the main or include script as a new level. If the file is missing,
    /// everything stays unchanged (`false`, without a message). On success block comment is set to off,
    /// "active" to on and random branch to on (`0x00546994 ... 0x005469A1`); "branch chosen", roll and sum stay
    /// (D-343). Depth above 16: error (D-344).
    pub fn open(&mut self, path: &str) -> Result<bool, FormatError> {
        if self.levels.len() >= MAX_DEPTH {
            return Err(FormatError::Invalid(format!(
                "Map script: include depth above {MAX_DEPTH} at {path}"
            )));
        }
        let Some(bytes) = (self.read)(path) else {
            return Ok(false);
        };
        if self.levels.is_empty() {
            self.main_path = path.to_string();
        }
        self.levels.push(Level::new(path, &bytes));
        self.flags.comment = false;
        self.flags.active = true;
        self.flags.random_off = false;
        Ok(true)
    }

    /// Closes the top level (`GameFile_Close`); the flags stay as they are (D-343).
    pub fn close(&mut self) {
        self.levels.pop();
    }

    pub fn add_define(&mut self, name: &str) {
        self.defines.push(name.to_string());
    }

    pub fn add_const(&mut self, name: &str, value: &str) {
        self.consts.push((name.to_string(), value.to_string()));
    }

    pub fn defines(&self) -> &[String] {
        &self.defines
    }

    pub fn consts(&self) -> &[(String, String)] {
        &self.consts
    }

    /// Path of the main script (basis for [`include_path`], also in nested includes).
    pub fn main_path(&self) -> &str {
        &self.main_path
    }

    /// Path of the topmost open file, otherwise `""`.
    pub fn current_path(&self) -> &str {
        self.levels.last().map_or("", |level| &level.path)
    }

    /// Number of the line read last of the topmost file (1-based, skipped comment lines count; 0 before the
    /// first).
    pub fn line_number(&self) -> u32 {
        self.levels.last().map_or(0, |level| level.next as u32)
    }

    /// One processed line of the topmost open file (steps 1-7 of `FUN_005424B0`); `None` at the end of this
    /// file.
    pub fn next_line(&mut self) -> Option<String> {
        let raw = self.read_raw()?;
        Some(self.process(&raw))
    }

    /// Steps 1-2: read the line and carry the block comment along (`0x005424E2 ... 0x00542580`).
    fn read_raw(&mut self) -> Option<String> {
        let level = self.levels.last_mut()?;
        let Some(mut line) = level.read() else {
            self.flags.comment = false;
            return None;
        };
        if self.flags.comment {
            // Skip lines until one contains `*/`; that one continues. End of file in an open comment
            // switches it off (`0x00542537`), the line is dropped.
            while !line.contains("*/") {
                match level.read() {
                    Some(next) => line = next,
                    None => {
                        self.flags.comment = false;
                        return None;
                    }
                }
            }
            self.flags.comment = false;
        } else if line.contains("/*") && !line.contains("*/") {
            self.flags.comment = true;
        }
        Some(line)
    }

    /// Steps 3-7: TrimLeft, cutting off, `$` constant, directives, blanking.
    fn process(&mut self, raw: &str) -> String {
        // Cut off at the first `/`, `;` or `*` (`_mbscspn`, character set `0x00628164`).
        let trimmed = trim_left(raw);
        let end = trimmed.find(['/', ';', '*']).unwrap_or(trimmed.len());
        let mut line = trimmed[..end].to_string();

        // `$NAME` (`FUN_00542050`): only the first `$`, name = whole rest of the line, exact
        // comparison, first hit.
        if let Some(dollar) = line.find('$') {
            let name = &line[dollar + 1..];
            if let Some((_, value)) = self.consts.iter().find(|(known, _)| known == name) {
                line = format!("{} {value}", &line[..dollar]);
            }
        }

        if line.starts_with('#') {
            self.directive(&line);
        }

        // Blanking (`0x0054287E`): inactive or random branch off -> empty.
        if !self.flags.active || self.flags.random_off {
            line.clear();
        }
        line
    }

    /// `name in defines` (`FUN_00541570`, `_mbscmp` exact).
    fn is_defined(&self, name: &str) -> bool {
        self.defines.iter().any(|define| define == name)
    }

    /// Step 6: directive as a substring in fixed order (`0x00542634 ...`). The second word is the name
    /// (`sscanf("%s %s")`); if it is missing, the original reads a stack leftover, here `""`.
    fn directive(&mut self, line: &str) {
        let name = scan_word(line, 1).unwrap_or("");
        if line.contains("#ifdef") {
            let defined = self.is_defined(name);
            self.flags.active = defined;
            self.flags.chosen = defined;
        } else if line.contains("#ifndef") {
            let undefined = !self.is_defined(name);
            self.flags.active = undefined;
            self.flags.chosen = undefined;
        } else if line.contains("#elseif") {
            if self.flags.chosen {
                self.flags.active = false;
            } else {
                let defined = self.is_defined(name);
                self.flags.active = defined;
                self.flags.chosen = defined;
            }
        } else if line.contains("#else") {
            // `#elsif` does not contain this substring and has no effect (D-323).
            if self.flags.chosen {
                self.flags.active = false;
            } else {
                self.flags.active = true;
                self.flags.chosen = true;
            }
        } else if line.contains("#endif") {
            self.flags.active = true;
        } else if line.contains("#random") {
            self.flags.random_off = false;
            self.flags.roll = ((self.random)() % 100) as u32;
            self.flags.sum = 0;
        } else if let Some(position) = line.find("#chance") {
            // `sscanf("%s %i", ..., &c)`: if `%i` fails, c stays the position where it was found (0x00542803).
            let chance = scan_word(line, 1)
                .and_then(scan_i)
                .map_or(position as u32, |value| value as u32);
            self.flags.sum = self.flags.sum.wrapping_add(chance);
            if self.flags.roll > self.flags.sum {
                // `JA` unsigned (0x00542840): skip the branch.
                self.flags.random_off = true;
            } else {
                self.flags.roll = 101;
                self.flags.random_off = false;
            }
        } else if line.contains("#endrandom") {
            self.flags.random_off = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// Test host: files in memory, rolls from a list, counter of the calls.
    struct Host {
        files: BTreeMap<String, Vec<u8>>,
        rolls: Vec<i32>,
        draws: usize,
    }

    impl Host {
        fn new(files: &[(&str, &[u8])], rolls: &[i32]) -> Self {
            Host {
                files: files
                    .iter()
                    .map(|(p, b)| (p.to_string(), b.to_vec()))
                    .collect(),
                rolls: rolls.to_vec(),
                draws: 0,
            }
        }
    }

    /// Reads `main` and follows `#include "name"` recursively like the dispatcher: the include line
    /// itself does not appear in the output.
    fn run(
        host: &mut Host,
        main: &str,
        defines: &[&str],
        consts: &[(&str, &str)],
    ) -> Result<Vec<String>, FormatError> {
        let files = host.files.clone();
        let mut rolls = host.rolls.clone().into_iter();
        let mut draws = 0usize;
        let mut read = |p: &str| files.get(p).cloned();
        let mut random = || {
            draws += 1;
            rolls.next().unwrap_or(0)
        };
        let mut out = Vec::new();
        {
            let mut reader = ScriptReader::new(
                &mut read,
                &mut random,
                defines.iter().map(|d| d.to_string()).collect(),
            );
            for (n, v) in consts {
                reader.add_const(n, v);
            }
            assert!(reader.open(main)?, "main script missing");
            let result = read_level(&mut reader, &mut out);
            reader.close();
            result?;
        }
        host.draws = draws;
        Ok(out)
    }

    fn read_level(reader: &mut ScriptReader<'_>, out: &mut Vec<String>) -> Result<(), FormatError> {
        while let Some(line) = reader.next_line() {
            if let Some(name) = line
                .strip_prefix("#include \"")
                .and_then(|r| r.strip_suffix('"'))
            {
                let path = include_path(reader.main_path(), name);
                if reader.open(&path)? {
                    let result = read_level(reader, out);
                    reader.close();
                    result?;
                }
            } else {
                out.push(line);
            }
        }
        Ok(())
    }

    fn lines(script: &str, defines: &[&str]) -> Vec<String> {
        let mut host = Host::new(&[("m.rnd", script.as_bytes())], &[]);
        run(&mut host, "m.rnd", defines, &[]).unwrap()
    }

    fn lines_with_rolls(script: &str, rolls: &[i32]) -> Vec<String> {
        let mut host = Host::new(&[("m.rnd", script.as_bytes())], rolls);
        run(&mut host, "m.rnd", &[], &[]).unwrap()
    }

    #[test]
    fn line_reader_matches_gamefile_readline() {
        assert_eq!(lines("a\r\nb\n\nc", &[]), ["a", "b", "", "c"]);
        assert_eq!(lines("a\nb\n", &[]), ["a", "b"], "no empty remainder line");
        assert_eq!(lines("a\n\n", &[]), ["a", ""]);
        assert_eq!(lines("", &[]), Vec::<String>::new());
        assert_eq!(
            lines("x\ry\n", &[]),
            ["xy"],
            "\\r in the middle of the line is dropped"
        );
        assert_eq!(lines("  \tx  \n", &[]), ["x  "], "trimmed on the left only");
        assert_eq!(lines("\u{B}\u{C} y\n", &[]), ["y"], "also VT and FF");
        assert_eq!(
            lines("a\0b c\n", &[]),
            ["a"],
            "the line ends at the first NUL"
        );
        assert_eq!(
            lines("a\n\0", &[]),
            ["a", ""],
            "a line consisting only of NUL counts"
        );
        assert_eq!(
            lines("a\n\r", &[]),
            ["a"],
            "a single \\r at the end is not a line"
        );
    }

    #[test]
    fn line_reader_decodes_cp1252() {
        let mut host = Host::new(&[("m.rnd", b"\xE4\x80\n")], &[]);
        let out = run(&mut host, "m.rnd", &[], &[]).unwrap();
        assert_eq!(out, ["\u{E4}\u{20AC}"]);
    }

    // QUIRK D-321: kept
    #[test]
    fn cut_at_first_slash_semicolon_or_star() {
        assert_eq!(lines("numTiles 5 ; x", &[]), ["numTiles 5 "]);
        assert_eq!(lines("a*b", &[]), ["a"]);
        assert_eq!(lines("a/b", &[]), ["a"]);
        assert_eq!(lines("//SharePlayerLands", &[]), [""]);
        assert_eq!(lines("x ;y*z", &[]), ["x "]);
    }

    // QUIRK D-321: kept
    #[test]
    fn block_comment_rules() {
        assert_eq!(
            lines("x /* y\nskipped\n*also skipped\nz */ w\nafter", &[]),
            ["x ", "z ", "after"]
        );
        // `/* k */` in one line is not a block comment: cut off at the `/`.
        assert_eq!(lines("/* k */\nnext", &[]), ["", "next"]);
        // A line with `*/` without a preceding `/*` is an ordinary line.
        assert_eq!(lines("a */ b\nnext", &[]), ["a ", "next"]);
    }

    // QUIRK D-322: kept
    #[test]
    fn block_comment_swallows_directives_and_random_draws() {
        let mut host = Host::new(
            &[("m.rnd", b"/* c\n#random\n#ifdef X\n*/\n#random\nq\n")],
            &[7, 8],
        );
        let out = run(&mut host, "m.rnd", &[], &[]).unwrap();
        // `*/` alone becomes "" at the `*`; afterwards the text continues normally.
        assert_eq!(out, ["", "", "#random", "q"]);
        assert_eq!(host.draws, 1, "lines in the comment draw nothing (D-322)");
    }

    // QUIRK D-321: kept
    #[test]
    fn const_replaces_only_the_first_dollar() {
        let mut host = Host::new(&[("m.rnd", b"v $A\nv $B\nw $A $A\n$A\n")], &[]);
        let out = run(&mut host, "m.rnd", &[], &[("A", "5")]).unwrap();
        // `Left(pos) + " " + value`: "v " + " " + "5"; the name is the whole rest of the line,
        // so "$A $A" hits no entry; "$A" alone yields " 5".
        assert_eq!(out, ["v  5", "v $B", "w $A $A", " 5"]);
    }

    // QUIRK D-321: kept
    #[test]
    fn const_lookup_is_exact_and_first_match_wins() {
        let mut host = Host::new(&[("m.rnd", b"x $a\nx $A\n")], &[]);
        let out = run(&mut host, "m.rnd", &[], &[("A", "1"), ("A", "2")]).unwrap();
        assert_eq!(out, ["x $a", "x  1"]);
    }

    #[test]
    fn ifdef_is_not_nestable() {
        let script = "#ifdef KING_OF_THE_HILL\n#ifdef MAP_SIZE_SMALL\nx\n#endif\ny\n#endif\n";
        assert_eq!(
            lines(script, &["NORMAL"]),
            ["", "", "", "#endif", "y", "#endif"]
        );
        // With KING_OF_THE_HILL but without a size: the inner #ifdef switches off, the first
        // #endif switches on again.
        assert_eq!(
            lines(script, &["KING_OF_THE_HILL"]),
            ["#ifdef KING_OF_THE_HILL", "", "", "#endif", "y", "#endif"]
        );
    }

    // QUIRK D-323: kept
    #[test]
    fn elsif_is_no_directive() {
        // `#elsif` contains neither `#elseif` nor `#else` as a substring (Ghidra: `0x00628144` is
        // "#else"): the line has no effect, `b` stays in the switched-off branch.
        let script = "#ifdef NEVER\na\n#elsif X\nb\n#else\nc\n#endif\n";
        assert_eq!(lines(script, &[]), ["", "", "", "", "#else", "c", "#endif"]);
        // In the chosen branch `#elsif` stays as text and changes nothing.
        let script = "#ifdef A\na\n#elsif X\nb\n#else\nc\n#endif\n";
        assert_eq!(
            lines(script, &["A"]),
            ["#ifdef A", "a", "#elsif X", "b", "", "", "#endif"]
        );
    }

    #[test]
    fn ifndef_elseif_else_endif() {
        let script = "#ifndef A\nx\n#elseif B\ny\n#elseif A\nz\n#else\nw\n#endif\nv\n";
        assert_eq!(
            lines(script, &["A"]),
            ["", "", "", "", "#elseif A", "z", "", "", "#endif", "v"]
        );
        // Without defines the #ifndef chooses the first branch; later branches are off.
        assert_eq!(
            lines(script, &[]),
            ["#ifndef A", "x", "", "", "", "", "", "", "#endif", "v"]
        );
        // `#else` without a chosen branch switches on.
        assert_eq!(
            lines("#ifdef A\nx\n#else\ny\n#endif\n", &[]),
            ["", "", "#else", "y", "#endif"]
        );
    }

    // QUIRK D-350: deviation
    #[test]
    fn define_match_is_exact_and_missing_name_is_empty() {
        assert_eq!(
            lines("#ifdef MAP\nx\n#endif", &["MAP_SIZE"]),
            ["", "", "#endif"]
        );
        assert_eq!(lines("#ifdef map\nx\n#endif", &["MAP"]), ["", "", "#endif"]);
        // Without a name (heap leftover in the original, here ""): never defined.
        assert_eq!(lines("#ifdef\nx\n#endif", &["A"]), ["", "", "#endif"]);
        assert_eq!(
            lines("#ifndef\nx\n#endif", &["A"]),
            ["#ifndef", "x", "#endif"]
        );
    }

    #[test]
    fn directives_need_the_hash_in_column_zero() {
        // After TrimLeft the `#` is at position 0 even for indented directives.
        assert_eq!(lines("  #ifdef NEVER\nx\n", &[]), ["", ""]);
        // With a character in front of it, it is not a directive.
        assert_eq!(lines("a #ifdef NEVER\nx\n", &[]), ["a #ifdef NEVER", "x"]);
    }

    #[test]
    fn directive_is_found_as_substring_in_fixed_order() {
        // `#ifdef` comes before `#endif`: the line counts as `#ifdef` with the name "#endif".
        assert_eq!(lines("#ifdef #endif\nx\n", &[]), ["", ""]);
        // `#endrandom` does not contain `#random` (the `#` comes before `end`).
        assert_eq!(lines("#endrandom\nx\n", &[]), ["#endrandom", "x"]);
    }

    // QUIRK D-322: kept
    #[test]
    fn random_draws_in_inactive_branches() {
        let mut host = Host::new(&[("m.rnd", b"#ifdef NEVER\n#random\nx\n#endif\n")], &[40]);
        let out = run(&mut host, "m.rnd", &[], &[]).unwrap();
        assert_eq!(out, ["", "", "", "#endif"]);
        assert_eq!(
            host.draws, 1,
            "#random also draws in the inactive branch (D-322)"
        );

        let mut host = Host::new(
            &[("m.rnd", b"/*\n#random\n*/\n#random\n#random")],
            &[1, 2, 3],
        );
        run(&mut host, "m.rnd", &[], &[]).unwrap();
        assert_eq!(host.draws, 2, "no draw in the block comment");
    }

    // QUIRK D-322: kept
    #[test]
    fn random_roll_is_taken_modulo_100() {
        // Roll 150 -> 50: `#chance 50` hits (roll afterwards 101), `#chance 1` (sum 51) no longer does.
        let out = lines_with_rolls("#random\n#chance 50\na\n#chance 1\nb\n", &[150]);
        assert_eq!(out, ["#random", "#chance 50", "a", "", ""]);
        // Roll 100 -> 0: already hits the first branch with sum 0.
        let out = lines_with_rolls("#random\n#chance 0\na\n", &[100]);
        assert_eq!(out, ["#random", "#chance 0", "a"]);
    }

    // QUIRK D-322: kept
    #[test]
    fn chance_favours_the_first_branch() {
        let script = "#random\n#chance 50\na\n#chance 50\nb\n#endrandom\nc\n";
        // Roll 50: the first branch wins (50 <= 50), afterwards roll 101 > sum 100.
        assert_eq!(
            lines_with_rolls(script, &[50]),
            ["#random", "#chance 50", "a", "", "", "#endrandom", "c"]
        );
        // Roll 51: the second branch.
        assert_eq!(
            lines_with_rolls(script, &[51]),
            ["#random", "", "", "#chance 50", "b", "#endrandom", "c"]
        );
        // Roll 99 with sum 66: none.
        let script = "#random\n#chance 33\na\n#chance 33\nb\n#endrandom\nc\n";
        assert_eq!(
            lines_with_rolls(script, &[99]),
            ["#random", "", "", "", "", "#endrandom", "c"]
        );
        // A sum above 100 additionally opens later branches (the roll is 101 after the hit).
        let script = "#random\n#chance 60\na\n#chance 60\nb\n#endrandom\n";
        assert_eq!(
            lines_with_rolls(script, &[10]),
            [
                "#random",
                "#chance 60",
                "a",
                "#chance 60",
                "b",
                "#endrandom"
            ]
        );
    }

    #[test]
    fn chance_without_number_adds_its_position() {
        // sscanf("%s %i") fails: c stays the position where it was found (0 for `#` at position 0).
        let out = lines_with_rolls("#random\n#chance x\na\n", &[0]);
        assert_eq!(out, ["#random", "#chance x", "a"]);
        let out = lines_with_rolls("#random\n#chance x\na\n", &[1]);
        assert_eq!(out, ["#random", "", ""], "sum 0 < roll 1");
        // Hex like `%i`.
        let out = lines_with_rolls("#random\n#chance 0x10\na\n", &[16]);
        assert_eq!(out, ["#random", "#chance 0x10", "a"]);
    }

    // QUIRK D-322: kept
    #[test]
    fn negative_chance_wraps_unsigned() {
        // Sum -1 is unsigned 0xFFFFFFFF: every roll hits (`JA` unsigned).
        let out = lines_with_rolls("#random\n#chance -1\na\n", &[99]);
        assert_eq!(out, ["#random", "#chance -1", "a"]);
    }

    // QUIRK D-326: kept
    #[test]
    fn unset_flags_start_at_zero() {
        // D-326: branch chosen, roll and sum are 0 (heap leftovers in the original).
        assert_eq!(lines("#else\na\n", &[]), ["#else", "a"]);
        // `#chance` before `#random`: roll 0 <= sum 5.
        assert_eq!(lines("#chance 5\na\n", &[]), ["#chance 5", "a"]);
    }

    // QUIRK D-343: kept
    #[test]
    fn include_resets_flags_and_does_not_restore_them() {
        let mut host = Host::new(
            &[
                ("d/m.rnd", b"a\n#include \"inc.rnd\"\nb\n"),
                ("d/inc.rnd", b"#ifdef NEVER\nfoo\n"),
            ],
            &[],
        );
        let out = run(&mut host, "d/m.rnd", &[], &[]).unwrap();
        // The include ends in the switched-off branch, without #endif: `b` stays empty (D-343).
        assert_eq!(out, ["a", "", "", ""]);
    }

    // QUIRK D-343: kept
    #[test]
    fn include_keeps_the_chosen_flag() {
        // `#ifdef A` sets "branch chosen"; the include only resets "active", so its `#else` switches off
        // (D-343).
        let mut host = Host::new(
            &[
                ("d/m.rnd", b"#ifdef A\n#include \"inc.rnd\"\nb\n"),
                ("d/inc.rnd", b"#else\nq\n"),
            ],
            &[],
        );
        let out = run(&mut host, "d/m.rnd", &["A"], &[]).unwrap();
        assert_eq!(out, ["#ifdef A", "", "", ""]);
    }

    // QUIRK D-321: kept
    #[test]
    fn include_ending_in_a_block_comment_leaves_no_comment() {
        // End of file in an open block comment switches it off (`0x00542537`).
        let mut host = Host::new(
            &[
                ("d/m.rnd", b"a\n#include \"inc.rnd\"\nb\n"),
                ("d/inc.rnd", b"/* x\ny"),
            ],
            &[],
        );
        let out = run(&mut host, "d/m.rnd", &[], &[]).unwrap();
        // The line `/* x` itself continues as "", `y` is dropped, the end switches the comment off.
        assert_eq!(out, ["a", "", "b"]);
    }

    // QUIRK D-344: deviation
    #[test]
    fn include_depth_is_limited() {
        let mut host = Host::new(&[("d/x.rnd", b"#include \"x.rnd\"\n")], &[]);
        let err = run(&mut host, "d/x.rnd", &[], &[]).unwrap_err();
        let text = err.to_string();
        assert!(text.contains("include depth"), "{text}");
        assert!(text.contains("d/x.rnd"), "{text}");
    }

    // QUIRK D-344: deviation
    #[test]
    fn sixteen_levels_are_allowed() {
        // Main + 15 includes = depth 16.
        let mut files: Vec<(String, Vec<u8>)> = Vec::new();
        for i in 0..16 {
            let body = if i < 15 {
                format!("#include \"f{}.rnd\"\nl{i}\n", i + 1)
            } else {
                format!("l{i}\n")
            };
            files.push((format!("d/f{i}.rnd"), body.into_bytes()));
        }
        let refs: Vec<(&str, &[u8])> = files
            .iter()
            .map(|(p, b)| (p.as_str(), b.as_slice()))
            .collect();
        let mut host = Host::new(&refs, &[]);
        let out = run(&mut host, "d/f0.rnd", &[], &[]).unwrap();
        assert_eq!(out.len(), 16);
        assert_eq!(out[0], "l15");
        // A seventeenth level fails.
        files[15].1 = b"#include \"f16.rnd\"\n".to_vec();
        files.push(("d/f16.rnd".into(), b"x\n".to_vec()));
        let refs: Vec<(&str, &[u8])> = files
            .iter()
            .map(|(p, b)| (p.as_str(), b.as_slice()))
            .collect();
        let mut host = Host::new(&refs, &[]);
        assert!(run(&mut host, "d/f0.rnd", &[], &[]).is_err());
    }

    #[test]
    fn missing_include_is_not_an_error() {
        let mut host = Host::new(&[("d/m.rnd", b"a\n#include \"nope.rnd\"\nb\n")], &[]);
        let out = run(&mut host, "d/m.rnd", &[], &[]).unwrap();
        assert_eq!(out, ["a", "b"]);
    }

    #[test]
    fn include_path_is_relative_to_the_main_script() {
        assert_eq!(
            include_path("data/randommapscripts/x.rnd", "Textures.inc"),
            "data/randommapscripts/Textures.inc"
        );
        assert_eq!(include_path("x.rnd", "Textures.inc"), "");
        // First the last `/`; only without `/` the last `\`.
        assert_eq!(include_path("a\\b\\x.rnd", "t.inc"), "a\\b\\t.inc");
        assert_eq!(include_path("a/b\\x.rnd", "t.inc"), "a/t.inc");
    }

    #[test]
    fn nested_include_uses_the_main_script_directory() {
        let mut host = Host::new(
            &[
                ("d/m.rnd", b"#include \"a.inc\"\n"),
                ("d/a.inc", b"#include \"b.inc\"\nfrom_a\n"),
                ("d/b.inc", b"from_b\n"),
            ],
            &[],
        );
        assert_eq!(
            run(&mut host, "d/m.rnd", &[], &[]).unwrap(),
            ["from_b", "from_a"]
        );
    }

    #[test]
    fn open_reports_missing_file_and_tracks_position() {
        let files: BTreeMap<String, Vec<u8>> =
            [("m.rnd".to_string(), b"a\n/* c\nd\n*/ e\nf".to_vec())].into();
        let mut read = |p: &str| files.get(p).cloned();
        let mut random = || 0;
        let mut reader = ScriptReader::new(&mut read, &mut random, vec!["X".into()]);
        assert_eq!(reader.current_path(), "");
        assert!(!reader.open("nope.rnd").unwrap());
        assert_eq!(reader.current_path(), "");
        assert!(reader.open("m.rnd").unwrap());
        assert_eq!(reader.current_path(), "m.rnd");
        assert_eq!(reader.main_path(), "m.rnd");
        assert_eq!(reader.line_number(), 0);
        assert_eq!(reader.next_line().as_deref(), Some("a"));
        assert_eq!(reader.line_number(), 1);
        // Line 2 `/* c` switches the comment on and continues ("" after cutting off).
        assert_eq!(reader.next_line().as_deref(), Some(""));
        assert_eq!(reader.line_number(), 2);
        // Line 3 is skipped, line 4 `*/ e` ends the comment and becomes ""; the skipped line
        // counts.
        assert_eq!(reader.next_line().as_deref(), Some(""));
        assert_eq!(reader.line_number(), 4);
        assert_eq!(reader.next_line().as_deref(), Some("f"));
        assert_eq!(reader.next_line(), None);
        assert_eq!(reader.next_line(), None, "end stays end until close()");
        reader.close();
        assert_eq!(reader.current_path(), "");
        assert_eq!(reader.next_line(), None);
        reader.add_define("Y");
        reader.add_const("K", "v");
        assert_eq!(reader.defines(), ["X", "Y"]);
        assert_eq!(reader.consts(), [("K".to_string(), "v".to_string())]);
    }
}
