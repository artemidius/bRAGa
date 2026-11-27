// SPDX-License-Identifier: Apache-2.0

//! A scripted loopback HTTP stub for the adapter tests.

use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};

/// One canned HTTP response.
pub(crate) struct Reply {
    pub(crate) status: u16,
    pub(crate) body: String,
    pub(crate) delay: Option<Duration>,
}

impl Reply {
    /// A `200` response carrying `body`.
    pub(crate) fn json(body: impl Into<String>) -> Self {
        Self { status: 200, body: body.into(), delay: None }
    }

    /// A response with `status` and an empty body.
    pub(crate) fn status(status: u16) -> Self {
        Self { status, body: String::new(), delay: None }
    }

    /// Delays the response; used to exercise the client timeout.
    pub(crate) fn delayed(mut self, delay: Duration) -> Self {
        self.delay = Some(delay);
        self
    }
}

/// A `127.0.0.1:0` HTTP server that answers each request with the handler's scripted reply.
pub(crate) struct Stub {
    addr: SocketAddr,
    requests: Arc<AtomicUsize>,
}

impl Stub {
    /// Binds an ephemeral port and serves requests on a detached thread.
    pub(crate) fn start(handler: impl Fn(usize, &str) -> Reply + Send + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind stub listener");
        let addr = listener.local_addr().expect("stub local address");
        let requests = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&requests);
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                serve_connection(stream, &handler, &counter);
            }
        });
        Self { addr, requests }
    }

    /// The endpoint URL for `path` on this stub.
    pub(crate) fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    /// Number of requests received so far.
    pub(crate) fn requests(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }
}

fn serve_connection(
    mut stream: TcpStream,
    handler: &impl Fn(usize, &str) -> Reply,
    requests: &AtomicUsize,
) {
    let Ok(reader_stream) = stream.try_clone() else { return };
    let mut reader = BufReader::new(reader_stream);
    while let Some(body) = read_request(&mut reader) {
        let index = requests.fetch_add(1, Ordering::SeqCst);
        let reply = handler(index, &body);
        if let Some(delay) = reply.delay {
            thread::sleep(delay);
        }
        if write_reply(&mut stream, &reply).is_err() {
            break;
        }
    }
}

fn read_request(reader: &mut BufReader<TcpStream>) -> Option<String> {
    let mut content_length = 0_usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            return None;
        }
        if line == "\r\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            content_length = value.trim().parse().ok()?;
        }
    }
    let mut body = vec![0_u8; content_length];
    reader.read_exact(&mut body).ok()?;
    String::from_utf8(body).ok()
}

fn write_reply(stream: &mut TcpStream, reply: &Reply) -> std::io::Result<()> {
    let reason = match reply.status {
        200 => "OK",
        401 => "Unauthorized",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        _ => "Status",
    };
    write!(
        stream,
        "HTTP/1.1 {} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        reply.status,
        reply.body.len(),
        reply.body
    )?;
    stream.flush()
}
