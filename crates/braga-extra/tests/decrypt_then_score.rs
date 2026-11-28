// SPDX-License-Identifier: AGPL-3.0-or-later

//! Decrypt-then-score must return the same top-k as the base `braga::retrieval::query` contract on
//! a fixed corpus, and it must never use the in-SQL KNN path.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use braga::{
    config::{Config, Embedding, Scan, Storage},
    embedding::{EmbeddingPipeline, Metric},
    retrieval,
};
use braga_chunker::{ContentHash, content_hash};
use braga_embedder::{EmbedError, Embedder};
use braga_extra::{ExtraKey, XChachaEncryptor, query};
use braga_storage::{Key, NewChunk, Store, StoreError};
use tempfile::TempDir;

const MODEL: &str = "fake";
const DIM: u32 = 4;

fn db_key() -> Key {
    Key::from_bytes([0x5A; 32])
}

fn master() -> ExtraKey {
    ExtraKey::from_bytes([0x11; 32])
}

fn temp_db() -> (TempDir, PathBuf) {
    let dir = TempDir::new().expect("temporary directory");
    let path = dir.path().join("index.db");
    (dir, path)
}

fn open_base(path: &Path) -> Store {
    Store::open(path, &db_key()).expect("open base store")
}

fn open_extra(path: &Path) -> Store {
    Store::builder()
        .with_encryptor(Arc::new(XChachaEncryptor::new(master())))
        .open(path, &db_key())
        .expect("open extra store")
}

fn config(path: &Path) -> Config {
    Config {
        scan: Scan { dir: path.parent().expect("parent").to_path_buf(), ..Scan::default() },
        embedding: Embedding::default(),
        storage: Storage { path: path.to_path_buf() },
    }
}

/// Seeds one file with `(line_start, line_end, content)` chunks, keyed by content hash.
fn seed_file(store: &Store, path: &str, chunks: &[(i64, i64, &str)]) {
    let ids: Vec<ContentHash> =
        chunks.iter().map(|(_, _, content)| content_hash(content.as_bytes())).collect();
    let rows: Vec<NewChunk<'_>> = chunks
        .iter()
        .zip(&ids)
        .map(|((line_start, line_end, content), id)| NewChunk {
            id: id.as_bytes(),
            byte_offset: 0,
            line_start: *line_start,
            line_end: *line_end,
            content,
        })
        .collect();
    store.sync_file(path, &[0x77; 32], &rows).expect("seed chunks");
}

/// Indexes `chunks` with the embedding pipeline.
fn index(store: &Store, embedder: &dyn Embedder, chunks: &[(i64, i64, &str)]) {
    let items: Vec<(ContentHash, &str)> =
        chunks.iter().map(|(_, _, content)| (content_hash(content.as_bytes()), *content)).collect();
    EmbeddingPipeline::new(Metric::Cosine).run(store, embedder, &items).expect("index chunks");
}

/// A fixed-width embedder with a per-text vector table, so fixtures control the geometry.
struct FakeEmbedder {
    model: String,
    vectors: HashMap<String, Vec<f32>>,
}

impl FakeEmbedder {
    fn new(vectors: &[(&str, [f32; 4])]) -> Self {
        Self {
            model: MODEL.to_owned(),
            vectors: vectors
                .iter()
                .map(|(text, vector)| ((*text).to_owned(), vector.to_vec()))
                .collect(),
        }
    }
}

impl Embedder for FakeEmbedder {
    fn embed(&self, inputs: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError> {
        Ok(inputs
            .iter()
            .map(|text| self.vectors.get(*text).cloned().unwrap_or_else(|| vec![0.5; 4]))
            .collect())
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn dim(&self) -> Option<usize> {
        Some(4)
    }
}

#[test]
fn decrypt_then_score_matches_the_base_top_k() {
    let (_base_dir, base_path) = temp_db();
    let (_extra_dir, extra_path) = temp_db();
    let base = open_base(&base_path);
    let extra = open_extra(&extra_path);

    let a_chunks = [(1, 2, "alpha text"), (3, 4, "needle")];
    let b_chunks = [(5, 6, "beta text")];
    let all = [a_chunks[0], a_chunks[1], b_chunks[0]];
    seed_file(&base, "docs/a.md", &a_chunks);
    seed_file(&base, "docs/b.md", &b_chunks);
    seed_file(&extra, "docs/a.md", &a_chunks);
    seed_file(&extra, "docs/b.md", &b_chunks);

    let embedder = FakeEmbedder::new(&[
        ("alpha text", [1.0, 0.0, 0.0, 0.0]),
        ("needle", [0.96, 0.28, 0.0, 0.0]),
        ("beta text", [0.0, 1.0, 0.0, 0.0]),
    ]);
    index(&base, &embedder, &all);
    index(&extra, &embedder, &all);

    let base_hits = retrieval::query(&base, &embedder, "needle", 10).expect("base query");
    let extra_hits = query(&config(&extra_path), &db_key(), &master(), &embedder, "needle", 10)
        .expect("extra query");

    let base_order: Vec<&str> = base_hits.iter().map(|hit| hit.content.as_str()).collect();
    let extra_order: Vec<&str> = extra_hits.iter().map(|hit| hit.content.as_str()).collect();
    assert_eq!(extra_order, ["needle", "alpha text", "beta text"]);
    assert_eq!(extra_order, base_order);

    let base_where: Vec<_> =
        base_hits.iter().map(|hit| (hit.path.clone(), hit.line_start, hit.line_end)).collect();
    let extra_where: Vec<_> =
        extra_hits.iter().map(|hit| (hit.path.clone(), hit.line_start, hit.line_end)).collect();
    assert_eq!(extra_where, base_where);
    assert_eq!(extra_hits[0].line_start, 3);
    assert_eq!(extra_hits[0].line_end, 4);

    assert_eq!(
        query(&config(&extra_path), &db_key(), &master(), &embedder, "needle", 2)
            .expect("limited query")
            .len(),
        2
    );
    let empty = query(&config(&extra_path), &db_key(), &master(), &embedder, "needle", 0)
        .expect("empty query");
    assert_eq!(empty, Vec::new());
}

#[test]
fn the_extra_store_fails_closed_on_in_sql_knn() {
    let (_dir, path) = temp_db();
    let store = open_extra(&path);
    assert!(matches!(store.vector_search(DIM, &[1.0; 4], 1), Err(StoreError::SeamUnavailable)));
}
