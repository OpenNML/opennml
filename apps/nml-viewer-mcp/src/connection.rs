//! Connection to the control channel of a viewer: one JSON line out per request, one back.

use std::fmt;
use std::time::Duration;

use nml_control::{Command, MAX_RESPONSE_LINE, Outcome, Request, Response, VIEWER_GONE};
use serde::de::DeserializeOwned;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};

/// Maximum duration of a request. Before the level is loaded the viewer answers only `hello`;
/// everything else waits until the window and GPU are ready.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// Error of a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestError {
    /// The viewer rejected the command with this message (the connection stays usable).
    Viewer(String),
    /// Connection cut or disturbed; the viewer has probably terminated.
    Lost(String),
}

impl fmt::Display for RequestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Viewer(message) => write!(f, "{message}"),
            Self::Lost(reason) => write!(f, "Connection to the viewer lost: {reason}"),
        }
    }
}

pub struct Connection {
    reader: BufReader<OwnedReadHalf>,
    writer: OwnedWriteHalf,
    next_id: u64,
}

impl Connection {
    /// Connects to the control channel on `127.0.0.1:<port>`.
    pub async fn connect(port: u16) -> std::io::Result<Self> {
        let stream = TcpStream::connect(("127.0.0.1", port)).await?;
        let (reader, writer) = stream.into_split();
        Ok(Self {
            reader: BufReader::new(reader),
            writer,
            next_id: 1,
        })
    }

    /// Sends `command` and waits for the response (result as a JSON value).
    pub async fn request(&mut self, command: Command) -> Result<serde_json::Value, RequestError> {
        let id = self.next_id;
        self.next_id += 1;
        let mut line = serde_json::to_vec(&Request { id, command })
            .map_err(|e| RequestError::Lost(format!("request not serializable: {e}")))?;
        line.push(b'\n');
        let exchange = async {
            self.writer.write_all(&line).await?;
            let mut buf = Vec::new();
            (&mut self.reader)
                .take(MAX_RESPONSE_LINE as u64 + 1)
                .read_until(b'\n', &mut buf)
                .await?;
            Ok::<_, std::io::Error>(buf)
        };
        let buf = tokio::time::timeout(REQUEST_TIMEOUT, exchange)
            .await
            .map_err(|_| {
                RequestError::Lost(format!(
                    "no response within {} s",
                    REQUEST_TIMEOUT.as_secs()
                ))
            })?
            .map_err(|e| RequestError::Lost(e.to_string()))?;
        if buf.last() != Some(&b'\n') {
            return Err(RequestError::Lost(if buf.is_empty() {
                "connection closed".into()
            } else {
                "response aborted or too long".into()
            }));
        }
        let response: Response = serde_json::from_slice(&buf)
            .map_err(|e| RequestError::Lost(format!("response not readable: {e}")))?;
        if response.id != id {
            return Err(RequestError::Lost(format!(
                "response with ID {} to request {id}",
                response.id
            )));
        }
        match response.outcome {
            Outcome::Ok(value) => Ok(value),
            // The event loop has ended; the control channel closes right away.
            Outcome::Error(message) if message == VIEWER_GONE => Err(RequestError::Lost(message)),
            Outcome::Error(message) => Err(RequestError::Viewer(message)),
        }
    }

    /// Like [`Connection::request`], result as `T`.
    pub async fn request_as<T: DeserializeOwned>(
        &mut self,
        command: Command,
    ) -> Result<T, RequestError> {
        let value = self.request(command).await?;
        serde_json::from_value(value)
            .map_err(|e| RequestError::Lost(format!("response has an unexpected shape: {e}")))
    }
}
