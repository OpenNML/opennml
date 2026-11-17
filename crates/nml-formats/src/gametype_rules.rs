//! Rule files `gametypevictory.ini` and `gametypedefeat.ini`: per game type (0 to 6) and nation (0 to 5) one
//! rule tree of `OR`/`AND` blocks with leaves, read like `GameEngine_LoadGameTypeConditionsIni`
//! (`0x0042CE60`) and the recursive block reader `0x0042C9E0` (no function in Ghidra), see
//! `docs/formats/gifcon-misc.md` and re/ingame-menus 6.1. Reading and data types only; the
//! evaluation (`GameTypeRules_Evaluate` `0x0042C900`) is done by `nml-sim`.

use crate::config::{game_lines, left, lower_german, mid, trim_left};
use crate::property::atol;
use crate::sound::NATIONS;

/// Paths in the game.
pub const GAMETYPE_VICTORY_PATH: &str = "data/gifcon/gametypevictory.ini";
pub const GAMETYPE_DEFEAT_PATH: &str = "data/gifcon/gametypedefeat.ini";

/// Number of game types (0 to 6) and nations (0 to 5) of the table (`GameTypeRules_Evaluate`).
pub const GAMETYPE_COUNT: usize = 7;
pub const NATION_COUNT: usize = 6;

/// Leaf of a rule tree (`FUN_0042C520`, leaf type 0 to 8 in the order of the keywords
/// `0x0061899C` ... `0x00618914`). Numbers as the original reads them (see [`parse_gametype_rules`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleLeaf {
    /// `MATCHLOWERTHAN match limit` (type 0): number of objects for the match `< limit`.
    MatchLowerThan(i32, i32),
    /// `MATCHGREATERTHAN match limit` (type 1): count `> limit`.
    MatchGreaterThan(i32, i32),
    /// `RESOURCELOWERTHAN kind limit` (type 2): resource `< limit`.
    ResourceLowerThan(i32, i32),
    /// `RESOURCEGREATERTHAN kind limit` (type 3): resource `> limit`.
    ResourceGreaterThan(i32, i32),
    /// `CHECKGUIDTRIGGER guid bit` (type 4).
    CheckGuidTrigger(i32, i32),
    /// `CHECKPLAYERID slot` (type 5): slot `== slot`.
    CheckPlayerId(i32),
    /// `CHECKTIMER number` (type 6): remaining time of the timer `< 1 s`, missing timer true.
    CheckTimer(i32),
    /// `CHECKENEMY` (type 7).
    CheckEnemy,
    /// `CHECKFLAGOWNER` (type 8).
    CheckFlagOwner,
}

/// Node of a rule tree. `Or`/`And` like `FUN_0042C820` (leaf type 10000 or `0x2711`): an empty `Or`
/// is false, an empty `And` true. `Not` only arises from the negation sign `!` before a block
/// (see [`parse_gametype_rules`], O-198).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleNode {
    Or(Vec<RuleNode>),
    And(Vec<RuleNode>),
    Not(Box<RuleNode>),
    Leaf(RuleLeaf),
}

/// Table `[game type][nation]` (`GameEngine+8 + (nation + 6*game type)*4`); `None` = no root
/// (`GameTypeRules_Evaluate` then returns false). Nation 0 to 5 = `WID`, `PRI`, `SPA`, `SID`, `ENG`,
/// `PAT` (`dropdown.ini`, list 0).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GameTypeRules {
    pub rules: [[Option<RuleNode>; NATION_COUNT]; GAMETYPE_COUNT],
}

/// Keywords of the leaves in the order of the leaf type (table `0x0066FEAC`, filled from the
/// strings `0x0061899C` ... `0x00618914`) with the number of parameters (`0x006188A4`).
const LEAF_KEYS: [(&str, usize); 9] = [
    ("MATCHLOWERTHAN", 2),
    ("MATCHGREATERTHAN", 2),
    ("RESOURCELOWERTHAN", 2),
    ("RESOURCEGREATERTHAN", 2),
    ("CHECKGUIDTRIGGER", 2),
    ("CHECKPLAYERID", 1),
    ("CHECKTIMER", 1),
    ("CHECKENEMY", 0),
    ("CHECKFLAGOWNER", 0),
];

/// Leaf from a line (`0x0042C960` and the parameter part of `0x0042C9E0`): the first keyword
/// the line begins with (`Left(length)`, case matters; more characters after it do not matter).
/// For each parameter the original takes the rest after the first space of the remaining line
/// (only at position > 0) and reads it with `atol`; if the space is missing, the remaining line
/// stays and `atol` reads the same number once more (O-199).
fn parse_leaf(line: &str) -> Option<RuleLeaf> {
    let (kind, &(_, count)) = LEAF_KEYS
        .iter()
        .enumerate()
        .find(|(_, (key, _))| left(line, key.chars().count()) == *key)?;
    let mut rest = line.to_string();
    let mut params = [0i32; 2];
    for param in params.iter_mut().take(count) {
        if let Some(pos) = rest.chars().position(|c| c == ' ')
            && pos > 0
        {
            rest = mid(&rest, pos + 1).to_string();
        }
        *param = atol(&rest) as i32;
    }
    let [a, b] = params;
    Some(match kind {
        0 => RuleLeaf::MatchLowerThan(a, b),
        1 => RuleLeaf::MatchGreaterThan(a, b),
        2 => RuleLeaf::ResourceLowerThan(a, b),
        3 => RuleLeaf::ResourceGreaterThan(a, b),
        4 => RuleLeaf::CheckGuidTrigger(a, b),
        5 => RuleLeaf::CheckPlayerId(a),
        6 => RuleLeaf::CheckTimer(a),
        7 => RuleLeaf::CheckEnemy,
        _ => RuleLeaf::CheckFlagOwner,
    })
}

/// Nation index of an abbreviation: list 0 of `dropdown.ini` (`Dropdown_LookupValue` `0x00481230`:
/// lowercased, exact comparison without trimming). Unknown = `None` (the original returns a
/// pointer value as the index there).
fn nation_index(name: &str) -> Option<usize> {
    let name = lower_german(name);
    NATIONS.iter().position(|n| lower_german(n) == name)
}

#[derive(Clone, Copy, PartialEq)]
enum Block {
    Or,
    And,
}

struct Reader {
    lines: Vec<String>,
    next: usize,
}

impl Reader {
    /// `GameFile_ReadLine`; `None` at the end of the file.
    fn read(&mut self) -> Option<String> {
        let line = self.lines.get(self.next)?.clone();
        self.next += 1;
        Some(line)
    }

    /// Recursive block reader `0x0042C9E0`: reads up to a `/OR` or `/AND` (whichever word opened
    /// the block); `None` if the file ends before (return value 0 of the original, then the reading
    /// of all outer blocks ends too).
    ///
    /// `negate` is the flag `+0xC` of the block (`!` before `OR`/`AND`). `FUN_0042C820` applies
    /// it to the result of **each child**, not to that of the block itself (O-198); here it
    /// therefore appears as `Not` around each child. A `!` before a leaf never has an effect.
    fn block(&mut self, kind: Block, negate: bool) -> Option<RuleNode> {
        let mut children = Vec::new();
        loop {
            let raw = self.read()?;
            let line = trim_left(&raw);
            let (negated, line) = match line.strip_prefix('!') {
                Some(rest) => (true, rest),
                None => (false, line),
            };
            match line {
                "OR" => children.push(self.block(Block::Or, negated)?),
                "AND" => children.push(self.block(Block::And, negated)?),
                "/OR" | "/AND" => break,
                _ => children.extend(parse_leaf(line).map(RuleNode::Leaf)),
            }
        }
        if negate {
            children = children
                .into_iter()
                .map(|child| RuleNode::Not(Box::new(child)))
                .collect();
        }
        Some(match kind {
            Block::Or => RuleNode::Or(children),
            Block::And => RuleNode::And(children),
        })
    }
}

/// Reads a rule file like `GameEngine_LoadGameTypeConditionsIni` (`0x0042CE60`).
///
/// - At the top level only `GAMETYPE n` (`atol`), `/GAMETYPE` (no effect), `NATION <abbreviation>`,
///   `/NATION`, `OR` and `AND` (without `!`) apply; every other word, including a leaf, is skipped. A
///   `/OR` or `/AND` at the top level ends the reading of the file.
/// - `/NATION` enters the last read root into `[game type][nation]`. Several roots per
///   `NATION`: the last wins; a `NATION` without a root of its own inherits the previous one of the file (the
///   original keeps it in a variable that carries it over, O-200). Before the first root there is none: there the
///   original stores a pointer value from the stack, here the slot stays empty.
/// - A game type outside 0 to 6 and an unknown nation are written by the original to a wrong place of the
///   table (memory behind it); skipped here. No file of the shipped data contains this.
/// - If the file ends in an open block, the original reports a load error; here the function returns the
///   entries complete up to then (this does not occur in the shipped data).
pub fn parse_gametype_rules(text: &str) -> GameTypeRules {
    let mut reader = Reader {
        lines: game_lines(text),
        next: 0,
    };
    let mut rules = GameTypeRules::default();
    let mut gametype: Option<usize> = None;
    let mut nation: Option<usize> = None;
    let mut root: Option<RuleNode> = None;
    while let Some(raw) = reader.read() {
        let line = trim_left(&raw);
        if left(line, 9) == "GAMETYPE " {
            gametype = usize::try_from(atol(mid(line, 9)))
                .ok()
                .filter(|&g| g < GAMETYPE_COUNT);
        } else if line == "/GAMETYPE" {
        } else if left(line, 7) == "NATION " {
            nation = nation_index(mid(line, 7));
        } else if line == "/NATION" {
            if let (Some(g), Some(n), Some(root)) = (gametype, nation, &root) {
                rules.rules[g][n] = Some(root.clone());
            }
        } else if line == "OR" || line == "AND" {
            let kind = if line == "OR" { Block::Or } else { Block::And };
            match reader.block(kind, false) {
                Some(node) => root = Some(node),
                None => break,
            }
        } else if line == "/OR" || line == "/AND" {
            break;
        }
    }
    rules
}

#[cfg(test)]
mod tests {
    use super::*;
    use RuleLeaf::*;
    use RuleNode::*;

    fn leaf(l: RuleLeaf) -> RuleNode {
        Leaf(l)
    }

    /// Game type 1 of the defeat file, written as in the shipped data (CRLF, tabs).
    const KILL_HERO: &str = "#\r\n# GAMETYPE 1 KILLHERO\r\n#\r\n\r\nGAMETYPE 1\r\nNATION SPA\r\nOR\r\n\
        \tMATCHLOWERTHAN 2720 1\r\n\tAND\r\n\t\tMATCHLOWERTHAN 2 1\r\n\t\tMATCHLOWERTHAN 8 1\r\n\t/AND\r\n\
        /OR\r\n/NATION\r\n\r\nNATION WID\r\nOR\r\n\tMATCHLOWERTHAN 2721 1\r\n\tAND\r\n\
        \t\tMATCHLOWERTHAN 2 1\r\n\t\tMATCHLOWERTHAN 8 1\r\n\t/AND\r\n/OR\r\n/NATION\r\n/GAMETYPE\r\n";

    fn kill_hero(hero: i32) -> RuleNode {
        Or(vec![
            leaf(MatchLowerThan(hero, 1)),
            And(vec![leaf(MatchLowerThan(2, 1)), leaf(MatchLowerThan(8, 1))]),
        ])
    }

    #[test]
    fn parses_nested_and_or_with_leaves() {
        let rules = parse_gametype_rules(KILL_HERO);
        assert_eq!(rules.rules[1][2], Some(kill_hero(2720)), "SPA");
        assert_eq!(rules.rules[1][0], Some(kill_hero(2721)), "WID");
        let filled = rules.rules.iter().flatten().flatten().count();
        assert_eq!(filled, 2);
    }

    #[test]
    fn unknown_nation_and_gametype_are_skipped() {
        let text = "GAMETYPE 0\nNATION XXX\nAND\nCHECKENEMY\n/AND\n/NATION\n\
                    NATION pat\nOR\nCHECKENEMY\n/OR\n/NATION\n/GAMETYPE\n\
                    GAMETYPE 7\nNATION SPA\nOR\nCHECKENEMY\n/OR\n/NATION\n/GAMETYPE\n\
                    GAMETYPE -1\nNATION SPA\nOR\nCHECKENEMY\n/OR\n/NATION\n/GAMETYPE\n";
        let rules = parse_gametype_rules(text);
        // `XXX` is missing in `dropdown.ini`; `pat` counts lowercased (Dropdown_LookupValue).
        assert_eq!(rules.rules[0][5], Some(Or(vec![leaf(CheckEnemy)])));
        assert_eq!(rules.rules.iter().flatten().flatten().count(), 1);
    }

    #[test]
    fn leaf_without_numbers_defaults_like_the_original() {
        // The block reader takes the rest after the first space for each number and calls `atol`;
        // without a space the line stays and `atol` reads the same line again.
        let text = "GAMETYPE 0\nNATION WID\nAND\nMATCHLOWERTHAN\nMATCHGREATERTHAN 8\n\
                    RESOURCELOWERTHAN 3 5\nRESOURCEGREATERTHAN  4  6\nCHECKGUIDTRIGGER 2613\n\
                    CHECKPLAYERID\nCHECKTIMER\nCHECKENEMY\nCHECKFLAGOWNER\n/AND\n/NATION\n/GAMETYPE\n";
        let rules = parse_gametype_rules(text);
        assert_eq!(
            rules.rules[0][0],
            Some(And(vec![
                leaf(MatchLowerThan(0, 0)),
                // one number: the second pass reads the same remaining line "8" once more
                leaf(MatchGreaterThan(8, 8)),
                leaf(ResourceLowerThan(3, 5)),
                // double space: position 0 of the remainder does not count as a separator
                leaf(ResourceGreaterThan(4, 4)),
                leaf(CheckGuidTrigger(2613, 2613)),
                leaf(CheckPlayerId(0)),
                leaf(CheckTimer(0)),
                leaf(CheckEnemy),
                leaf(CheckFlagOwner),
            ]))
        );
    }

    #[test]
    fn negation_negates_the_children_of_a_block_and_leaves_ignore_it() {
        // `FUN_0042C820` applies the flag +0xC of the block to the results of its children;
        // a leaf never evaluates it (O-198).
        let text = "GAMETYPE 2\nNATION SID\nOR\n!AND\nCHECKENEMY\n!CHECKFLAGOWNER\n/AND\n!CHECKTIMER 0\n/OR\n/NATION\n";
        let rules = parse_gametype_rules(text);
        assert_eq!(
            rules.rules[2][3],
            Some(Or(vec![
                And(vec![
                    Not(Box::new(leaf(CheckEnemy))),
                    Not(Box::new(leaf(CheckFlagOwner))),
                ]),
                leaf(CheckTimer(0)),
            ]))
        );
    }

    #[test]
    fn last_root_wins_and_a_nation_without_root_reuses_the_previous_one() {
        let text = "GAMETYPE 3\nNATION WID\nOR\nCHECKENEMY\n/OR\nAND\nCHECKFLAGOWNER\n/AND\n/NATION\n\
                    NATION PRI\n/NATION\n/GAMETYPE\n";
        let rules = parse_gametype_rules(text);
        let last = Some(And(vec![leaf(CheckFlagOwner)]));
        assert_eq!(rules.rules[3][0], last);
        assert_eq!(rules.rules[3][1], last);
    }

    #[test]
    fn top_level_closing_word_ends_reading_and_blocks_close_on_either_word() {
        let text = "GAMETYPE 0\nNATION WID\nOR\nCHECKENEMY\n/AND\n/NATION\n/OR\n\
                    NATION PRI\nAND\n/AND\n/NATION\n";
        let rules = parse_gametype_rules(text);
        assert_eq!(rules.rules[0][0], Some(Or(vec![leaf(CheckEnemy)])));
        // nothing is read any more after the surplus `/OR` at the top level
        assert_eq!(rules.rules[0][1], None);
    }

    #[test]
    fn empty_blocks_and_stray_lines_are_kept_or_ignored() {
        let text = "GAMETYPE 4\nNATION ENG\nAND\n# CHECKENEMY\nUNKNOWN 3\n\nOR\n/OR\nAND\n/AND\n/AND\n/NATION\n";
        let rules = parse_gametype_rules(text);
        assert_eq!(rules.rules[4][4], Some(And(vec![Or(vec![]), And(vec![])])));
    }
}
