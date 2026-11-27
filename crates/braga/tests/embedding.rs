// SPDX-License-Identifier: Apache-2.0

//! Integration tests for the embedding pipeline: cache dedup, dimension pinning, metric pinning,
//! and `build_embedder` provider mapping over a real encrypted [`Store`].
//!
//! The fixed-width embedder is a test double; the storage and HTTP seams are the real
//! implementations (`SQLCipher` + `sqlite-vec`, and `ureq` against a loopback stub).

use std::{
    io::{BufRead as _, BufReader, Read as _, Write as _},
    net::TcpListener,
    path::{Path, PathBuf},
    sync::Mutex,
    thread,
};

use braga::embedding::{EmbeddingError, EmbeddingPipeline, Metric, build_embedder};
use braga::{config::Embedding, embedding::EmbeddingOutcome};
use braga_chunker::{ContentHash, content_hash};
use braga_embedder::{EmbedError, Embedder};
use braga_storage::{Key, NewChunk, Store, StoreError};
use rusqlite::Connection;
use tempfile::TempDir;

const MODEL: &str = "fake";

fn temp_db() -> (TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("create temporary directory");
    let path = dir.path().join("braga.sqlite3");
    (dir, path)
}

fn open_store(path: &Path) -> Store {
    Store::open(path, &Key::from_bytes([0x5A; 32])).expect("open encrypted store")
}

fn open_raw(path: &Path) -> Connection {
    let conn = Connection::open(path).expect("open raw encrypted database");
    conn.key(None::<&str>, Key::from_bytes([0x5A; 32]).as_bytes(), false).expect("apply key");
    conn
}

fn table_exists(conn: &Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
        [name],
        |row| row.get(0),
    )
    .expect("inspect sqlite_master")
}

/// Seeds one file with one chunk per text and returns the chunk handles in input order.
fn seed<'a>(store: &Store, texts: &[&'a str]) -> Vec<(ContentHash, &'a str)> {
    let chunks: Vec<(ContentHash, &'a str)> =
        texts.iter().map(|text| (content_hash(text.as_bytes()), *text)).collect();
    let rows: Vec<NewChunk<'_>> = chunks
        .iter()
        .map(|(id, text)| NewChunk {
            id: id.as_bytes(),
            byte_offset: 0,
            line_start: 1,
            line_end: 1,
            content: text,
        })
        .collect();
    store.sync_file("seed.md", &[0xBB; 32], &rows).expect("seed chunks");
    chunks
}

/// A fixed-width embedder that records every call, so cache dedup is observable.
struct FakeEmbedder {
    model: String,
    width: usize,
    dim: Mutex<Option<usize>>,
    calls: Mutex<usize>,
    seen: Mutex<Vec<String>>,
}

impl FakeEmbedder {
    fn new(width: usize) -> Self {
        Self {
            model: MODEL.to_owned(),
            width,
            dim: Mutex::new(None),
            calls: Mutex::new(0),
            seen: Mutex::new(Vec::new()),
        }
    }

    fn calls(&self) -> usize {
        *self.calls.lock().expect("calls lock")
    }

    fn seen(&self) -> Vec<String> {
        self.seen.lock().expect("seen lock").clone()
    }

    /// Simulates a fresh process: the adapter no longer remembers the width it learned.
    fn forget_dim(&self) {
        *self.dim.lock().expect("dim lock") = None;
    }
}

impl Embedder for FakeEmbedder {
    fn embed(&self, inputs: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError> {
        *self.calls.lock().expect("calls lock") += 1;
        self.seen.lock().expect("seen lock").extend(inputs.iter().map(|text| (*text).to_owned()));
        *self.dim.lock().expect("dim lock") = Some(self.width);
        Ok(inputs
            .iter()
            .map(|text| {
                let mut vector = vec![0.5_f32; self.width];
                vector[0] = f32::from(*text.as_bytes().first().unwrap_or(&0));
                vector
            })
            .collect())
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn dim(&self) -> Option<usize> {
        *self.dim.lock().expect("dim lock")
    }
}

#[test]
fn first_run_embeds_every_chunk_and_pins_the_shape() {
    let (_dir, path) = temp_db();
    let store = open_store(&path);
    let texts = ["alpha", "beta"];
    let chunks = seed(&store, &texts);
    let embedder = FakeEmbedder::new(4);

    let outcome =
        EmbeddingPipeline::new(Metric::Cosine).run(&store, &embedder, &chunks).expect("first run");

    assert_eq!(
        outcome,
        EmbeddingOutcome { cache_hits: 0, embedded: 2, model: MODEL.to_owned(), dim: 4 }
    );
    assert_eq!(embedder.calls(), 1);
    assert_eq!(
        store.registered_model(MODEL).expect("read registry"),
        Some((4, "cosine".to_owned()))
    );
    assert!(
        table_exists(&open_raw(&path), "embeddings_d4"),
        "the pipeline must activate the dimension's vec0 table"
    );
    for (id, _) in &chunks {
        assert!(
            store.embedding_exists(MODEL, 4, id.as_bytes()).expect("cache lookup"),
            "every chunk must have a stored embedding"
        );
    }
}

#[test]
fn second_run_hits_the_cache_without_calling_the_embedder() {
    let (_dir, path) = temp_db();
    let store = open_store(&path);
    let chunks = seed(&store, &["alpha", "beta"]);
    let embedder = FakeEmbedder::new(4);
    let pipeline = EmbeddingPipeline::new(Metric::Cosine);

    pipeline.run(&store, &embedder, &chunks).expect("first run");
    let outcome = pipeline.run(&store, &embedder, &chunks).expect("second run");

    assert_eq!(
        outcome,
        EmbeddingOutcome { cache_hits: 2, embedded: 0, model: MODEL.to_owned(), dim: 4 }
    );
    assert_eq!(embedder.calls(), 1, "a fully cached run must not call the provider");
}

#[test]
fn only_the_missing_chunk_is_embedded() {
    let (_dir, path) = temp_db();
    let store = open_store(&path);
    let chunks = seed(&store, &["alpha", "beta"]);
    let embedder = FakeEmbedder::new(4);

    store.ensure_embedding_table(4, "cosine").expect("create embeddings_d4");
    store.register_model(MODEL, 4, "cosine").expect("register model");
    store
        .insert_embedding(chunks[0].0.as_bytes(), MODEL, 4, &[1.0, 2.0, 3.0, 4.0])
        .expect("pre-embed the first chunk");

    let outcome = EmbeddingPipeline::new(Metric::Cosine)
        .run(&store, &embedder, &chunks)
        .expect("partial run");

    assert_eq!(
        outcome,
        EmbeddingOutcome { cache_hits: 1, embedded: 1, model: MODEL.to_owned(), dim: 4 }
    );
    assert_eq!(embedder.seen(), vec!["beta".to_owned()]);
}

#[test]
fn a_fresh_process_recovers_the_dimension_from_the_registry() {
    let (_dir, path) = temp_db();
    let store = open_store(&path);
    let chunks = seed(&store, &["alpha", "beta"]);
    let embedder = FakeEmbedder::new(4);
    let pipeline = EmbeddingPipeline::new(Metric::Cosine);

    pipeline.run(&store, &embedder, &chunks).expect("first run");
    embedder.forget_dim();
    let outcome = pipeline.run(&store, &embedder, &chunks).expect("run after a restart");

    assert_eq!(
        outcome,
        EmbeddingOutcome { cache_hits: 2, embedded: 0, model: MODEL.to_owned(), dim: 4 },
        "the registry, not the adapter's lost cache, supplies the dimension"
    );
    assert_eq!(embedder.calls(), 1);
}

#[test]
fn a_metric_change_fails_closed() {
    let (_dir, path) = temp_db();
    let store = open_store(&path);
    let chunks = seed(&store, &["alpha"]);
    let embedder = FakeEmbedder::new(4);

    EmbeddingPipeline::new(Metric::Cosine).run(&store, &embedder, &chunks).expect("first run");
    let error = EmbeddingPipeline::new(Metric::L2)
        .run(&store, &embedder, &chunks)
        .expect_err("a different metric must not reuse the collection");

    assert!(matches!(
        error,
        EmbeddingError::Store(StoreError::ModelConflict { model }) if model == MODEL
    ));
    assert_eq!(embedder.calls(), 1, "the pin must be checked before embedding");
}

/// Serves one canned JSON response on a loopback socket and returns the endpoint URL.
fn serve_json(body: &'static str) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind stub server");
    let address = listener.local_addr().expect("stub address");
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept request");
        let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
        let mut content_length = 0;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).expect("read header");
            if line == "\r\n" || line.is_empty() {
                break;
            }
            let lower = line.to_ascii_lowercase();
            if let Some(value) = lower.strip_prefix("content-length:") {
                content_length = value.trim().parse().expect("content length");
            }
        }
        let mut request_body = vec![0_u8; content_length];
        reader.read_exact(&mut request_body).expect("read request body");
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).expect("write response");
    });
    (format!("http://{address}/embed"), handle)
}

#[test]
fn build_embedder_maps_each_provider_to_its_adapter() {
    let cases = [
        ("ollama", r#"{"embeddings":[[1.0,2.0,3.0]]}"#),
        ("openai", r#"{"data":[{"index":0,"embedding":[1.0,2.0,3.0]}]}"#),
        ("openai-compatible", r#"{"data":[{"index":0,"embedding":[1.0,2.0,3.0]}]}"#),
    ];

    for (provider, body) in cases {
        let (url, server) = serve_json(body);
        let embedding =
            Embedding { provider: provider.to_owned(), url, model: "model-a".to_owned() };
        let embedder = build_embedder(&embedding).expect("provider should build");

        let vectors = embedder.embed(&["hello"]).expect("the adapter's response shape must match");
        assert_eq!(vectors, vec![vec![1.0, 2.0, 3.0]], "{provider}");
        assert_eq!(embedder.dim(), Some(3), "{provider}");
        server.join().expect("stub server exits");
    }
}
