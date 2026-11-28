// SPDX-License-Identifier: Apache-2.0

//! Shared subprocess, configuration, and loopback-stub helpers for the CLI integration tests.

#![allow(dead_code)]

use std::{
    fs,
    io::{BufRead, BufReader, Read, Write as _},
    net::{SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Command, Output},
    thread,
};

use serde_json::{Value, json};
use tempfile::TempDir;

/// Runs the built `braga` binary with a cleared environment and `HOME` set to `home`.
pub(crate) fn run_in(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_braga"))
        .env_clear()
        .env("HOME", home)
        .args(args)
        .output()
        .expect("run the braga binary")
}

/// Decodes captured output bytes for substring assertions.
pub(crate) fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Renders `bytes` as lowercase hex.
pub(crate) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let mut rendered = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(rendered, "{byte:02x}");
    }
    rendered
}

/// Sub-slice search, with a non-empty needle because `windows(0)` panics.
pub(crate) fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|window| window == needle)
}

/// A scan directory, config, key file, database path, and embedding stub in one temp directory.
pub(crate) struct Fixture {
    root: TempDir,
    config: PathBuf,
    key: PathBuf,
    stub: Stub,
}

impl Fixture {
    /// Creates the fixture with `files` under `<root>/docs` and a stub-backed `[embedding]`.
    pub(crate) fn new(files: &[(&str, &str)], key: [u8; 32]) -> Self {
        let root = tempfile::tempdir().expect("create temporary directory");
        let docs = root.path().join("docs");
        fs::create_dir(&docs).expect("create scan directory");
        for (name, content) in files {
            fs::write(docs.join(name), content).expect("write fixture file");
        }

        let db = root.path().join("index.db");
        let config = root.path().join("braga.toml");
        let key_path = root.path().join("key");
        fs::write(&key_path, key).expect("write key file");

        let stub = Stub::start();
        fs::write(&config, config_toml(&docs, &db, &stub.url("/v1/embeddings")))
            .expect("write config file");

        Self { root, config, key: key_path, stub }
    }

    /// Runs `braga` with `args` plus this fixture's `--config` and `--key-file`.
    pub(crate) fn run(&self, args: &[&str]) -> Output {
        let mut all: Vec<&str> = args.to_vec();
        all.extend(["--config", self.config.to_str().expect("config path is UTF-8")]);
        all.extend(["--key-file", self.key.to_str().expect("key path is UTF-8")]);
        run_in(self.root.path(), &all)
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

/// A `127.0.0.1:0` OpenAI-compatible embeddings stub that answers every input with one vector.
pub(crate) struct Stub {
    addr: SocketAddr,
}

impl Stub {
    /// Binds an ephemeral port and serves requests on a detached thread.
    pub(crate) fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind stub listener");
        let addr = listener.local_addr().expect("read stub address");
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                serve(stream);
            }
        });
        Self { addr }
    }

    /// The endpoint URL for `path` on this stub.
    pub(crate) fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
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
