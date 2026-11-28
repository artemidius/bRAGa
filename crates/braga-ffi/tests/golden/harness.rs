// SPDX-License-Identifier: Apache-2.0

//! A hermetic fixture: temporary tree, configuration, key, index, and a loopback embedding stub.

use std::{
    fs,
    io::{BufRead as _, BufReader, Read as _, Write as _},
    net::{SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    thread,
};

use serde_json::{Value, json};
use tempfile::TempDir;

/// The sample document the semantic round-trip indexes.
pub(crate) const SAMPLE: &str = "# Notes\n\nThe quick brown fox jumps over the lazy dog.\n";

/// The query text that must find [`SAMPLE`].
pub(crate) const QUERY: &str = "quick brown fox";

/// A fixture rooted in one temporary directory.
pub(crate) struct Fixture {
    root: TempDir,
    config: PathBuf,
    key: PathBuf,
}

impl Fixture {
    /// Creates `docs/notes.md`, a key file, and a stub-backed embedding configuration.
    pub(crate) fn new() -> Self {
        let root = tempfile::tempdir().expect("create temporary directory");
        let docs = root.path().join("docs");
        fs::create_dir(&docs).expect("create scan directory");
        fs::write(docs.join("notes.md"), SAMPLE).expect("write sample document");

        let db = root.path().join("index.db");
        let key = root.path().join("key");
        fs::write(&key, [0x5A_u8; 32]).expect("write key file");

        let stub = Stub::start();
        let config = root.path().join("braga.toml");
        fs::write(&config, config_toml(&docs, &db, &stub.url("/v1/embeddings")))
            .expect("write config file");

        Self { root, config, key }
    }

    /// The configuration path as UTF-8.
    pub(crate) fn config(&self) -> &str {
        self.config.to_str().expect("config path is UTF-8")
    }

    /// The key path as UTF-8.
    pub(crate) fn key(&self) -> &str {
        self.key.to_str().expect("key path is UTF-8")
    }

    /// Writes another file in the fixture root and returns its UTF-8 path.
    pub(crate) fn write(&self, name: &str, contents: &[u8]) -> String {
        let path = self.root.path().join(name);
        fs::write(&path, contents).expect("write fixture file");
        path.to_str().expect("fixture path is UTF-8").to_owned()
    }

    /// The root directory, for callers that need a fresh target path.
    pub(crate) fn root(&self) -> &Path {
        self.root.path()
    }
}

/// Renders a `braga.toml` that scans `docs`, stores in `db`, and embeds through `url`.
fn config_toml(docs: &Path, db: &Path, url: &str) -> String {
    format!(
        "[scan]\ndir = {:?}\n\n[embedding]\nprovider = \"openai-compatible\"\nurl = {:?}\nmodel = \"stub-embed\"\n\n[storage]\npath = {:?}\n",
        docs.display().to_string(),
        url,
        db.display().to_string()
    )
}

/// A `127.0.0.1:0` `OpenAI`-compatible embeddings stub that answers every input with one vector.
struct Stub {
    address: SocketAddr,
}

impl Stub {
    /// Binds an ephemeral port and serves requests on a detached thread.
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind stub listener");
        let address = listener.local_addr().expect("read stub address");
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                serve(stream);
            }
        });
        Self { address }
    }

    /// The endpoint URL for `path` on this stub.
    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.address)
    }
}

/// Answers one embeddings request with `[1.0, 0.0, 0.0, 0.0]` per input.
fn serve(mut stream: TcpStream) {
    let Ok(reader_stream) = stream.try_clone() else { return };
    let mut reader = BufReader::new(reader_stream);
    let Some(body) = read_request(&mut reader) else { return };

    let inputs = serde_json::from_str::<Value>(&body)
        .ok()
        .and_then(|request| request.get("input").and_then(Value::as_array).map(Vec::len))
        .unwrap_or(0);
    let data: Vec<Value> = (0..inputs)
        .map(|index| json!({"index": index, "embedding": [1.0, 0.0, 0.0, 0.0]}))
        .collect();
    let response = json!({"data": data}).to_string();

    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",
        response.len()
    );
    let _ = stream.flush();
}

/// Reads one HTTP request body using its `Content-Length`.
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
