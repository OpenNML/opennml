//! Line codec: one JSON line per message, `\n` as the end (a `\r` before it is removed).

use std::io::{self, BufRead, Write};

use serde::Serialize;

use crate::protocol::{Request, Response};

/// Maximum length of a request line (without line end).
pub const MAX_REQUEST_LINE: usize = 1 << 20;

/// Maximum length of a response line when reading in the MCP server (screenshots as Base64).
pub const MAX_RESPONSE_LINE: usize = 256 << 20;

/// Result of [`read_line`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LineRead {
    /// A complete line without line end.
    Line(Vec<u8>),
    /// The line was longer than the limit; it was discarded up to the line end.
    TooLong,
    /// Connection closed (also in the middle of a line).
    Eof,
}

/// Reads a line of at most `limit` bytes. Longer lines are skipped up to their end and
/// reported as [`LineRead::TooLong`], without buffering more than `limit` bytes.
pub fn read_line(reader: &mut impl BufRead, limit: usize) -> io::Result<LineRead> {
    let mut line = Vec::new();
    let mut too_long = false;
    loop {
        let available = match reader.fill_buf() {
            Ok(buf) => buf,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if available.is_empty() {
            return Ok(LineRead::Eof);
        }
        let (chunk, done) = match available.iter().position(|&b| b == b'\n') {
            Some(i) => (&available[..i], i + 1),
            None => (available, available.len()),
        };
        let found_end = done > chunk.len();
        if !too_long {
            if line.len() + chunk.len() > limit {
                too_long = true;
                line = Vec::new();
            } else {
                line.extend_from_slice(chunk);
            }
        }
        reader.consume(done);
        if found_end {
            if too_long {
                return Ok(LineRead::TooLong);
            }
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            return Ok(LineRead::Line(line));
        }
    }
}

/// Writes `value` as one JSON line and flushes the buffer.
pub fn write_line(writer: &mut impl Write, value: &impl Serialize) -> io::Result<()> {
    serde_json::to_writer(&mut *writer, value)?;
    writer.write_all(b"\n")?;
    writer.flush()
}

/// Parses a request line; on an error the response to it (with the ID of the line,
/// if readable, otherwise 0).
pub fn parse_request(line: &[u8]) -> Result<Request, Response> {
    serde_json::from_slice(line).map_err(|e| {
        let id = serde_json::from_slice::<serde_json::Value>(line)
            .ok()
            .and_then(|v| v.get("id")?.as_u64())
            .unwrap_or(0);
        Response::error(id, format!("request not readable: {e}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{Command, Outcome};
    use std::io::{BufReader, Cursor};

    fn lines(input: &[u8], limit: usize, capacity: usize) -> Vec<LineRead> {
        let mut reader = BufReader::with_capacity(capacity, Cursor::new(input.to_vec()));
        let mut out = Vec::new();
        loop {
            let read = read_line(&mut reader, limit).unwrap();
            let eof = read == LineRead::Eof;
            out.push(read);
            if eof {
                return out;
            }
        }
    }

    #[test]
    fn reads_lines_and_strips_line_ends() {
        let got = lines(b"a\r\nbc\n\nrest", 10, 2);
        assert_eq!(
            got,
            [
                LineRead::Line(b"a".to_vec()),
                LineRead::Line(b"bc".to_vec()),
                LineRead::Line(Vec::new()),
                LineRead::Eof,
            ]
        );
    }

    #[test]
    fn overlong_lines_are_skipped_up_to_their_end() {
        // Limit 4: "abcd" fits exactly, "abcde" does not; after that it continues normally.
        let got = lines(b"abcd\nabcde\nxy\n", 4, 3);
        assert_eq!(
            got,
            [
                LineRead::Line(b"abcd".to_vec()),
                LineRead::TooLong,
                LineRead::Line(b"xy".to_vec()),
                LineRead::Eof,
            ]
        );
        // 1 MiB + 1 byte over the real limit.
        let mut big = vec![b'x'; MAX_REQUEST_LINE + 1];
        big.extend_from_slice(b"\n{}\n");
        let got = lines(&big, MAX_REQUEST_LINE, 8192);
        assert_eq!(
            got,
            [
                LineRead::TooLong,
                LineRead::Line(b"{}".to_vec()),
                LineRead::Eof
            ]
        );
        let mut exact = vec![b'x'; MAX_REQUEST_LINE];
        exact.push(b'\n');
        assert_eq!(
            lines(&exact, MAX_REQUEST_LINE, 8192)[0],
            LineRead::Line(vec![b'x'; MAX_REQUEST_LINE])
        );
    }

    #[test]
    fn writes_one_json_line() {
        let mut out = Vec::new();
        write_line(&mut out, &Response::error(1, "x")).unwrap();
        assert_eq!(out, b"{\"id\":1,\"error\":\"x\"}\n");
    }

    #[test]
    fn parse_errors_keep_the_id_when_readable() {
        assert_eq!(
            parse_request(br#"{"id":9,"cmd":"state"}"#).unwrap().command,
            Command::State
        );
        let error = parse_request(br#"{"id":9,"cmd":"fly"}"#).unwrap_err();
        assert_eq!(error.id, 9);
        assert!(
            matches!(&error.outcome, Outcome::Error(m) if m.contains("not readable")),
            "{error:?}"
        );
        assert_eq!(parse_request(b"no json").unwrap_err().id, 0);
        assert_eq!(parse_request(&[0xFF, 0xFE]).unwrap_err().id, 0);
    }
}
