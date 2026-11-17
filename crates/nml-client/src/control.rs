//! Control channel (`--control`): a thread accepts TCP connections one after another, reads one
//! request per line and passes it to the main thread as a [`ControlEvent`]; it writes the response
//! back as soon as the command has been executed there (`docs/viewer-control.md`).

use std::io::{self, BufReader, BufWriter};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use nml_control::{
    Command, LineRead, MAX_REQUEST_LINE, Request, Response, VIEWER_GONE, parse_request, read_line,
    write_line,
};
use winit::event_loop::EventLoopProxy;

/// A command for the main thread and the return path for its response.
#[derive(Debug)]
pub struct ControlEvent {
    pub request: Request,
    pub reply: mpsc::Sender<Response>,
}

/// The main thread no longer accepts commands (event loop ended).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Closed;

/// Path to the main thread: in the viewer the `EventLoopProxy` (wakes the event loop even
/// while it is waiting), in tests a channel.
pub trait ControlSink: Send + 'static {
    fn send(&self, event: ControlEvent) -> Result<(), Closed>;
}

impl ControlSink for EventLoopProxy<ControlEvent> {
    fn send(&self, event: ControlEvent) -> Result<(), Closed> {
        self.send_event(event).map_err(|_| Closed)
    }
}

impl ControlSink for mpsc::Sender<ControlEvent> {
    fn send(&self, event: ControlEvent) -> Result<(), Closed> {
        mpsc::Sender::send(self, event).map_err(|_| Closed)
    }
}

/// Running control channel.
pub struct ControlServer {
    addr: SocketAddr,
    /// Reports that the response to `quit` has been written.
    quit_written: mpsc::Receiver<()>,
}

/// Binds `addr` and starts the control thread. An error while binding aborts the start.
pub fn start(addr: SocketAddr, sink: impl ControlSink) -> io::Result<ControlServer> {
    let listener = TcpListener::bind(addr)?;
    let addr = listener.local_addr()?;
    let (quit_tx, quit_written) = mpsc::channel();
    thread::Builder::new()
        .name("control-channel".into())
        .spawn(move || serve(listener, sink, quit_tx))?;
    Ok(ControlServer { addr, quit_written })
}

impl ControlServer {
    /// Actual address (for port 0, the port chosen by the operating system).
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Waits at most `timeout` until the response to `quit` has been written (otherwise the
    /// process would end before it reaches the client).
    pub fn wait_quit_written(&self, timeout: Duration) -> bool {
        self.quit_written.recv_timeout(timeout).is_ok()
    }
}

/// Accepts connections one after another (others wait in the backlog) until the main thread is gone.
fn serve(listener: TcpListener, sink: impl ControlSink, quit_tx: mpsc::Sender<()>) {
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                if !serve_connection(stream, &sink, &quit_tx) {
                    return;
                }
            }
            Err(e) => {
                log::warn!("Control channel: connection not accepted: {e}");
                // Persistent errors (e.g. no handles left) must not loop endlessly.
                thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

/// Serves a connection until its end; `false` if the main thread no longer
/// responds (then the control channel has ended as well).
fn serve_connection(
    stream: TcpStream,
    sink: &impl ControlSink,
    quit_tx: &mpsc::Sender<()>,
) -> bool {
    let Ok(read_half) = stream.try_clone() else {
        return true;
    };
    let mut reader = BufReader::new(read_half);
    let mut writer = BufWriter::new(stream);
    loop {
        let response = match read_line(&mut reader, MAX_REQUEST_LINE) {
            Ok(LineRead::Eof) | Err(_) => return true,
            Ok(LineRead::TooLong) => Response::error(
                0,
                format!("request longer than {MAX_REQUEST_LINE} bytes, discarded"),
            ),
            Ok(LineRead::Line(line)) => match parse_request(&line) {
                Err(response) => response,
                Ok(request) => {
                    let (id, quit) = (request.id, request.command == Command::Quit);
                    let (reply, answer) = mpsc::channel();
                    let response = match sink.send(ControlEvent { request, reply }) {
                        Ok(()) => answer.recv().ok(),
                        Err(Closed) => None,
                    };
                    let Some(response) = response else {
                        let _ = write_line(&mut writer, &Response::error(id, VIEWER_GONE));
                        return false;
                    };
                    let written = write_line(&mut writer, &response).is_ok();
                    if quit {
                        let _ = quit_tx.send(());
                    }
                    if !written {
                        return true;
                    }
                    continue;
                }
            },
        };
        if write_line(&mut writer, &response).is_err() {
            return true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nml_control::Outcome;
    use std::io::{BufRead, Write};

    fn start_test() -> (ControlServer, mpsc::Receiver<ControlEvent>) {
        let (tx, rx) = mpsc::channel();
        let server = start("127.0.0.1:0".parse().unwrap(), tx).unwrap();
        (server, rx)
    }

    struct Client {
        reader: BufReader<TcpStream>,
        writer: TcpStream,
    }

    impl Client {
        fn connect(addr: SocketAddr) -> Self {
            let writer = TcpStream::connect(addr).unwrap();
            writer
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            Self {
                reader: BufReader::new(writer.try_clone().unwrap()),
                writer,
            }
        }

        fn send(&mut self, line: &[u8]) {
            self.writer.write_all(line).unwrap();
            self.writer.write_all(b"\n").unwrap();
        }

        /// Next response; `None` if the connection is closed.
        fn receive(&mut self) -> Option<Response> {
            let mut line = String::new();
            match self.reader.read_line(&mut line).unwrap() {
                0 => None,
                _ => Some(serde_json::from_str(&line).unwrap()),
            }
        }
    }

    /// Main thread: accepts a command and answers with `ok: <command name>`.
    fn answer(rx: &mpsc::Receiver<ControlEvent>) -> Command {
        let event = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let name = serde_json::to_value(&event.request.command).unwrap()["cmd"].clone();
        event
            .reply
            .send(Response::ok(event.request.id, name))
            .unwrap();
        event.request.command
    }

    fn error_text(response: &Response) -> &str {
        match &response.outcome {
            Outcome::Error(message) => message,
            Outcome::Ok(value) => panic!("expected an error, not {value}"),
        }
    }

    #[test]
    fn requests_are_answered_after_the_main_thread_ran_them() {
        let (server, rx) = start_test();
        let mut client = Client::connect(server.addr());
        client.send(br#"{"id":1,"cmd":"state"}"#);
        assert_eq!(answer(&rx), Command::State);
        assert_eq!(
            client.receive(),
            Some(Response::ok(1, "state")),
            "response carries the ID of the request"
        );
        client.send(br#"{"id":2,"cmd":"pause"}"#);
        assert_eq!(answer(&rx), Command::Pause);
        assert_eq!(client.receive(), Some(Response::ok(2, "pause")));
    }

    #[test]
    fn unreadable_requests_are_answered_by_the_thread_itself() {
        let (server, rx) = start_test();
        let mut client = Client::connect(server.addr());
        client.send(b"not json");
        let response = client.receive().unwrap();
        assert_eq!(response.id, 0);
        assert!(error_text(&response).contains("not readable"));
        client.send(br#"{"id":5,"cmd":"fly"}"#);
        let response = client.receive().unwrap();
        assert_eq!(response.id, 5);
        assert!(error_text(&response).contains("not readable"));
        assert!(rx.try_recv().is_err(), "nothing to the main thread");
    }

    #[test]
    fn overlong_lines_are_rejected_and_the_connection_stays_usable() {
        let (server, rx) = start_test();
        let mut client = Client::connect(server.addr());
        let writer = client.writer.try_clone().unwrap();
        // Write in a separate thread: 1 MiB does not fit into every socket buffer.
        let sender = thread::spawn(move || {
            let mut writer = writer;
            writer.write_all(&vec![b'x'; MAX_REQUEST_LINE + 1]).unwrap();
            writer
                .write_all(b"\n{\"id\":3,\"cmd\":\"hello\"}\n")
                .unwrap();
        });
        let response = client.receive().unwrap();
        assert!(error_text(&response).contains("longer than 1048576 bytes"));
        assert_eq!(answer(&rx), Command::Hello);
        assert_eq!(client.receive(), Some(Response::ok(3, "hello")));
        sender.join().unwrap();
    }

    #[test]
    fn a_vanished_main_thread_gives_an_error_and_ends_the_connection() {
        let (server, rx) = start_test();
        let mut client = Client::connect(server.addr());
        // Command accepted but never answered (event loop ended).
        client.send(br#"{"id":4,"cmd":"state"}"#);
        drop(rx.recv_timeout(Duration::from_secs(5)).unwrap());
        let response = client.receive().unwrap();
        assert_eq!(response.id, 4);
        assert_eq!(error_text(&response), "Viewer terminated");
        assert_eq!(client.receive(), None, "connection closed");
        // Main thread gone entirely: the same for a new connection.
        let (server, rx) = start_test();
        drop(rx);
        let mut client = Client::connect(server.addr());
        client.send(br#"{"id":6,"cmd":"hello"}"#);
        assert_eq!(error_text(&client.receive().unwrap()), "Viewer terminated");
        assert_eq!(client.receive(), None);
    }

    #[test]
    fn connections_are_served_one_after_another() {
        let (server, rx) = start_test();
        let mut first = Client::connect(server.addr());
        first.send(br#"{"id":1,"cmd":"state"}"#);
        assert_eq!(answer(&rx), Command::State);
        assert_eq!(first.receive(), Some(Response::ok(1, "state")));
        // The second connection waits until the first one is closed.
        let mut second = Client::connect(server.addr());
        second.send(br#"{"id":2,"cmd":"state"}"#);
        assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
        drop(first);
        assert_eq!(answer(&rx), Command::State);
        assert_eq!(second.receive(), Some(Response::ok(2, "state")));
    }

    #[test]
    fn quit_reports_when_its_answer_is_written() {
        let (server, rx) = start_test();
        let mut client = Client::connect(server.addr());
        assert!(!server.wait_quit_written(Duration::from_millis(10)));
        client.send(br#"{"id":9,"cmd":"quit"}"#);
        assert_eq!(answer(&rx), Command::Quit);
        assert_eq!(client.receive(), Some(Response::ok(9, "quit")));
        assert!(server.wait_quit_written(Duration::from_secs(5)));
    }
}
