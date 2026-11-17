//! Parser for the brace-based text formats (`.cfg`, `.lvl`, `.pef`).

use crate::FormatError;
use crate::error::syntax;

/// Decodes game text (Windows-1252).
pub fn decode_cp1252(bytes: &[u8]) -> String {
    encoding_rs::WINDOWS_1252
        .decode_without_bom_handling(bytes)
        .0
        .into_owned()
}

/// Characters of the bytes `0x80..=0x9F` in Windows-1252; `\0` stands for the five unassigned bytes
/// (`0x81`, `0x8D`, `0x8F`, `0x90`, `0x9D`), which `decode_cp1252` maps to U+0081 etc.
const CP1252_HIGH: [char; 32] = [
    '\u{20AC}', '\0', '\u{201A}', '\u{0192}', '\u{201E}', '\u{2026}', '\u{2020}', '\u{2021}',
    '\u{02C6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\0', '\u{017D}', '\0', '\0',
    '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}', '\u{2022}', '\u{2013}', '\u{2014}', '\u{02DC}',
    '\u{2122}', '\u{0161}', '\u{203A}', '\u{0153}', '\0', '\u{017E}', '\u{0178}',
];

/// Encodes text to Windows-1252, the inverse of [`decode_cp1252`]: the round trip
/// `decode_cp1252` -> `encode_cp1252` is lossless for all 256 bytes (including the unassigned
/// bytes `0x81`, `0x8D`, `0x8F`, `0x90`, `0x9D`). Characters that cannot be represented yield exactly one `?`
/// (`encoding_rs` would replace them with `&#...;`).
pub fn encode_cp1252(text: &str) -> Vec<u8> {
    text.chars()
        .map(|c| match c {
            '\u{0}'..='\u{7F}' | '\u{A0}'..='\u{FF}' => c as u8,
            '\u{81}' | '\u{8D}' | '\u{8F}' | '\u{90}' | '\u{9D}' => c as u8,
            _ => CP1252_HIGH
                .iter()
                .position(|&h| h == c && h != '\0')
                .map_or(b'?', |i| 0x80 + i as u8),
        })
        .collect()
}

/// A value: unquoted word or string in quotation marks.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Word(String),
    Str(String),
}

impl Value {
    pub fn as_str(&self) -> &str {
        match self {
            Value::Word(s) | Value::Str(s) => s,
        }
    }

    pub fn as_f32(&self) -> Option<f32> {
        self.as_str().parse().ok()
    }

    pub fn as_i64(&self) -> Option<i64> {
        self.as_str().parse().ok()
    }
}

/// A statement: key, arguments and optionally a `{ ... }` block.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub key: Value,
    pub args: Vec<Value>,
    pub children: Option<Vec<Node>>,
    /// Line number (from 1) for error messages.
    pub line: usize,
}

impl Node {
    pub fn name(&self) -> &str {
        self.key.as_str()
    }

    pub fn children(&self) -> &[Node] {
        self.children.as_deref().unwrap_or(&[])
    }

    /// First child with this key (ASCII case ignored).
    pub fn child(&self, key: &str) -> Option<&Node> {
        self.children()
            .iter()
            .find(|n| n.name().eq_ignore_ascii_case(key))
    }

    pub fn children_named<'a>(&'a self, key: &'a str) -> impl Iterator<Item = &'a Node> + 'a {
        self.children()
            .iter()
            .filter(move |n| n.name().eq_ignore_ascii_case(key))
    }

    pub fn arg(&self, index: usize) -> Option<&Value> {
        self.args.get(index)
    }

    /// Argument `index` as a value of type `T`; error with line number.
    pub fn parse_arg<T: std::str::FromStr>(&self, index: usize) -> Result<T, FormatError> {
        let value = self.args.get(index).ok_or_else(|| {
            syntax(
                self.line,
                &format!("{}: argument {} missing", self.name(), index + 1),
            )
        })?;
        value.as_str().parse().map_err(|_| {
            syntax(
                self.line,
                &format!(
                    "{}: \"{}\" is not a valid value",
                    self.name(),
                    value.as_str()
                ),
            )
        })
    }

    /// The first `N` arguments as values of type `T`.
    pub fn parse_args<T: std::str::FromStr, const N: usize>(&self) -> Result<[T; N], FormatError> {
        let values = (0..N)
            .map(|i| self.parse_arg(i))
            .collect::<Result<Vec<T>, _>>()?;
        Ok(values
            .try_into()
            .unwrap_or_else(|_| unreachable!("exactly N values")))
    }

    /// Argument `index` as text (word or string).
    pub fn str_arg(&self, index: usize) -> Result<&str, FormatError> {
        self.args.get(index).map(Value::as_str).ok_or_else(|| {
            syntax(
                self.line,
                &format!("{}: argument {} missing", self.name(), index + 1),
            )
        })
    }
}

/// All nodes with this key at any depth, in file order
/// (ASCII case ignored).
pub fn find_all<'a>(nodes: &'a [Node], key: &str) -> Vec<&'a Node> {
    let mut found = Vec::new();
    for node in nodes {
        if node.name().eq_ignore_ascii_case(key) {
            found.push(node);
        }
        found.extend(find_all(node.children(), key));
    }
    found
}

enum Token {
    Value(Value),
    Open,
    Close,
}

fn tokenize_line(line: &str, line_no: usize) -> Result<Vec<Token>, FormatError> {
    let mut tokens = Vec::new();
    let mut chars = line.char_indices().peekable();
    while let Some(&(start, c)) = chars.peek() {
        match c {
            c if c.is_whitespace() => {
                chars.next();
            }
            '{' => {
                chars.next();
                tokens.push(Token::Open);
            }
            '}' => {
                chars.next();
                tokens.push(Token::Close);
            }
            '"' => {
                chars.next();
                let end = loop {
                    match chars.next() {
                        Some((i, '"')) => break i,
                        Some(_) => {}
                        None => {
                            return Err(syntax(line_no, "string without closing quotation mark"));
                        }
                    }
                };
                tokens.push(Token::Value(Value::Str(line[start + 1..end].to_string())));
            }
            _ => {
                let rest = &line[start..];
                if tokens.is_empty() && (rest.starts_with('#') || rest.starts_with("//")) {
                    break;
                }
                let mut end = line.len();
                while let Some(&(i, c)) = chars.peek() {
                    if c.is_whitespace() || matches!(c, '{' | '}' | '"') {
                        end = i;
                        break;
                    }
                    chars.next();
                }
                tokens.push(Token::Value(Value::Word(line[start..end].to_string())));
            }
        }
    }
    Ok(tokens)
}

/// Closes the values collected on a line as a statement.
fn flush(level: &mut Vec<Node>, current: &mut Vec<Value>, line: usize) {
    if current.is_empty() {
        return;
    }
    let mut values = std::mem::take(current).into_iter();
    let key = values.next().unwrap();
    level.push(Node {
        key,
        args: values.collect(),
        children: None,
        line,
    });
}

/// Maximum permitted nesting depth of `{ ... }` blocks.
///
/// Deviation from the original (Rust only): `Node` is recursive, so `Drop`, `Clone`, `PartialEq`
/// and [`find_all`] run over the call stack. A broken file with tens of thousands of opened braces
/// would otherwise cause a stack overflow. The shipped data nests only a few levels deep at most.
const MAX_NESTING_DEPTH: usize = 64;

/// Parses a complete file into a list of top-level statements.
pub fn parse_blocks(text: &str) -> Result<Vec<Node>, FormatError> {
    let mut stack: Vec<Vec<Node>> = vec![Vec::new()];
    let mut open_lines: Vec<usize> = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line_no = index + 1;
        let mut current = Vec::new();
        for token in tokenize_line(line, line_no)? {
            match token {
                Token::Value(v) => current.push(v),
                Token::Open => {
                    flush(stack.last_mut().unwrap(), &mut current, line_no);
                    match stack.last().unwrap().last() {
                        Some(node) if node.children.is_none() => {}
                        _ => return Err(syntax(line_no, "\"{\" without preceding key")),
                    }
                    if open_lines.len() >= MAX_NESTING_DEPTH {
                        return Err(syntax(
                            line_no,
                            &format!("blocks nested deeper than {MAX_NESTING_DEPTH} levels"),
                        ));
                    }
                    stack.push(Vec::new());
                    open_lines.push(line_no);
                }
                Token::Close => {
                    flush(stack.last_mut().unwrap(), &mut current, line_no);
                    if stack.len() == 1 {
                        return Err(syntax(line_no, "\"}\" without opening brace"));
                    }
                    let children = stack.pop().unwrap();
                    open_lines.pop();
                    stack.last_mut().unwrap().last_mut().unwrap().children = Some(children);
                }
            }
        }
        flush(stack.last_mut().unwrap(), &mut current, line_no);
    }
    if let Some(&line) = open_lines.last() {
        return Err(syntax(line, "\"{\" is not closed"));
    }
    Ok(stack.pop().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(s: &str) -> Value {
        Value::Word(s.into())
    }

    #[test]
    fn cp1252_round_trip_is_lossless_for_all_bytes() {
        let all: Vec<u8> = (0..=255).collect();
        assert_eq!(encode_cp1252(&decode_cp1252(&all)), all);
    }

    #[test]
    fn unrepresentable_characters_become_one_question_mark() {
        assert_eq!(
            encode_cp1252("a\u{20AC}\u{E4}\u{A7}"),
            [b'a', 0x80, 0xE4, 0xA7]
        );
        assert_eq!(encode_cp1252("a\u{4E2D}b\u{1F600}"), b"a?b?");
        assert_eq!(encode_cp1252("\u{80}"), b"?");
    }

    #[test]
    fn parses_property_block_with_nested_list() {
        let text = "PROPERTY\r\n{\r\n\tGUID 1\r\n\tLABEL \"Unit is in the level:\"\r\n\tCHECKBOX\r\n\tDROPDOWN\r\n\t{\r\n\t\t\"enabled\"\r\n\t\t\"disabled\"\r\n\t}\r\n}\r\n";
        let nodes = parse_blocks(text).unwrap();
        assert_eq!(nodes.len(), 1);
        let prop = &nodes[0];
        assert_eq!(prop.name(), "PROPERTY");
        assert_eq!(
            prop.child("guid").unwrap().arg(0).unwrap().as_i64(),
            Some(1)
        );
        assert_eq!(
            prop.child("LABEL").unwrap().args,
            [Value::Str("Unit is in the level:".into())]
        );
        assert!(prop.child("CHECKBOX").unwrap().args.is_empty());
        let options: Vec<_> = prop
            .child("DROPDOWN")
            .unwrap()
            .children()
            .iter()
            .map(|n| n.name())
            .collect();
        assert_eq!(options, ["enabled", "disabled"]);
    }

    #[test]
    fn parses_level_sections_and_key_with_argument_before_block() {
        let text = "LEVELFILE V1.08e\n\n[RESOURCES]\n{\n\tCONFIG_GUID 0\n\t{\n\t\tCONFIG_LABEL\t\"Wood\"\n\t\tCONFIG_ENABLE\tTRUE\n\t}\n}\n[NEUTRAL]\n{\n[CONFIGDATA]\n{\n\tOBJECT_GUID 3602\n\t{\n\t}\n}\n}\n";
        let nodes = parse_blocks(text).unwrap();
        assert_eq!(nodes[0].name(), "LEVELFILE");
        assert_eq!(nodes[0].args, [word("V1.08e")]);
        let res = &nodes[1];
        assert_eq!(res.name(), "[RESOURCES]");
        let guid = res.child("CONFIG_GUID").unwrap();
        assert_eq!(guid.arg(0).unwrap().as_i64(), Some(0));
        assert_eq!(
            guid.child("CONFIG_LABEL").unwrap().arg(0).unwrap().as_str(),
            "Wood"
        );
        let obj = nodes[2]
            .child("[CONFIGDATA]")
            .unwrap()
            .child("OBJECT_GUID")
            .unwrap();
        assert_eq!(obj.arg(0).unwrap().as_i64(), Some(3602));
        assert_eq!(obj.children, Some(vec![]));
    }

    #[test]
    fn keeps_backslashes_and_umlauts_in_strings() {
        let nodes = parse_blocks("FILE \".\\data\\r3d\\wid\\x.rdo\"\nLABEL \"Gebäude\"\n").unwrap();
        assert_eq!(nodes[0].arg(0).unwrap().as_str(), r".\data\r3d\wid\x.rdo");
        assert_eq!(nodes[1].arg(0).unwrap().as_str(), "Gebäude");
    }

    #[test]
    fn parses_inline_braces_and_numbers() {
        let nodes = parse_blocks("A { B 1 -48.234482 }\n").unwrap();
        let b = nodes[0].child("B").unwrap();
        assert_eq!(b.arg(0).unwrap().as_i64(), Some(1));
        assert_eq!(b.arg(1).unwrap().as_f32(), Some(-48.234_48));
    }

    #[test]
    fn limits_nesting_depth() {
        let nested = |depth: usize| {
            "A
{
"
            .repeat(depth)
                + &"}
"
                .repeat(depth)
        };
        let nodes = parse_blocks(&nested(10)).unwrap();
        assert_eq!(nodes[0].children().len(), 1);
        assert!(parse_blocks(&nested(MAX_NESTING_DEPTH)).is_ok());
        // 100 levels: error on the line of the 65th opening (line 2 * 65).
        match parse_blocks(&nested(100)) {
            Err(FormatError::Syntax { line, .. }) => assert_eq!(line, 2 * (MAX_NESTING_DEPTH + 1)),
            other => panic!("syntax error expected, got {other:?}"),
        }
    }

    #[test]
    fn skips_comment_lines() {
        let nodes = parse_blocks("# Comment\n  // also a comment\nKEY 1\n").unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].name(), "KEY");
    }

    #[test]
    fn reports_syntax_errors_with_line_numbers() {
        let line_of = |text: &str| match parse_blocks(text) {
            Err(FormatError::Syntax { line, .. }) => line,
            other => panic!("syntax error expected, got {other:?}"),
        };
        assert_eq!(line_of("A\nB \"open\n"), 2);
        assert_eq!(line_of("A\n}\n"), 2);
        assert_eq!(line_of("A\n{\nB\n"), 2);
        assert_eq!(line_of("{\n}\n"), 1);
        assert_eq!(line_of("A\n{\n}\n{\n}\n"), 4);
    }

    #[test]
    fn typed_arguments_report_line_numbers() {
        let nodes = parse_blocks("A 1 2.5 x\n[B]\n{\n\tC \"text\"\n\t[B]\n\t{\n\t}\n}\n").unwrap();
        let a = &nodes[0];
        assert_eq!(a.parse_arg::<i32>(0).unwrap(), 1);
        assert_eq!(a.parse_args::<f32, 2>().unwrap(), [1.0, 2.5]);
        assert!(matches!(
            a.parse_arg::<f32>(2),
            Err(FormatError::Syntax { line: 1, .. })
        ));
        assert!(matches!(
            a.parse_arg::<i32>(5),
            Err(FormatError::Syntax { line: 1, .. })
        ));
        assert_eq!(nodes[1].child("C").unwrap().str_arg(0).unwrap(), "text");
        let found: Vec<usize> = find_all(&nodes, "[b]").iter().map(|n| n.line).collect();
        assert_eq!(found, [2, 5]);
    }

    #[test]
    fn decodes_cp1252() {
        assert_eq!(
            decode_cp1252(&[0x47, 0x65, 0x62, 0xE4, 0x75, 0x64, 0x65]),
            "Gebäude"
        );
    }
}
