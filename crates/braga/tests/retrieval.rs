// SPDX-License-Identifier: Apache-2.0

//! End-to-end tests for [`braga::retrieval::query`]: vector search, FTS5 search, and RRF fusion
//! over a real encrypted [`Store`] with a deterministic test embedder.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use braga::{
    embedding::{EmbeddingPipeline, Metric},
    retrieval::{QueryError, query},
};
use braga_chunker::{ContentHash, content_hash};
use braga_embedder::{EmbedError, Embedder};
use braga_storage::{Key, NewChunk, Store};
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
    width: usize,
    vectors: HashMap<String, Vec<f32>>,
}

impl FakeEmbedder {
    fn new(width: usize, vectors: &[(&str, &[f32])]) -> Self {
        Self {
            model: MODEL.to_owned(),
            width,
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
            .map(|text| self.vectors.get(*text).cloned().unwrap_or_else(|| vec![0.5; self.width]))
            .collect())
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn dim(&self) -> Option<usize> {
        Some(self.width)
    }
}

#[test]
fn query_returns_ranked_chunks_with_source_metadata() {
    let (_dir, path) = temp_db();
    let store = open_store(&path);
    let a_chunks = [(1, 2, "alpha text"), (3, 4, "needle")];
    let b_chunks = [(5, 6, "beta text")];
    seed_file(&store, "docs/a.md", &a_chunks);
    seed_file(&store, "docs/b.md", &b_chunks);
    let embedder = FakeEmbedder::new(
        4,
        &[
            ("alpha text", &[1.0, 0.0, 0.0, 0.0]),
            ("needle", &[0.96, 0.28, 0.0, 0.0]),
            ("beta text", &[0.0, 1.0, 0.0, 0.0]),
        ],
    );
    index(&store, &embedder, &[a_chunks[0], a_chunks[1], b_chunks[0]]);

    let results = query(&store, &embedder, "needle", 10).expect("query the index");

    assert_eq!(
        results.iter().map(|chunk| chunk.content.as_str()).collect::<Vec<_>>(),
        vec!["needle", "alpha text", "beta text"]
    );
    assert_eq!(results[0].path, "docs/a.md");
    assert_eq!((results[0].line_start, results[0].line_end), (3, 4));
    assert_eq!(results[0].chunk_id, content_hash(b"needle").as_bytes());
    assert_eq!(results[2].path, "docs/b.md");
    assert_eq!((results[2].line_start, results[2].line_end), (5, 6));
    assert!(
        results.windows(2).all(|pair| pair[0].score > pair[1].score),
        "scores must arrive in descending order"
    );
    assert!((results[0].score - 2.0 / 61.0).abs() < 1e-12, "the KNN and FTS rank-1 chunk fuses");
}

#[test]
fn a_chunk_matching_both_lists_outranks_a_vector_only_chunk() {
    let (_dir, path) = temp_db();
    let store = open_store(&path);
    let chunks = [(1, 1, "shared anchor"), (2, 2, "vector only")];
    seed_file(&store, "docs/rrf.md", &chunks);
    let embedder = FakeEmbedder::new(
        4,
        &[
            ("shared anchor", &[1.0, 0.0, 0.0, 0.0]),
            ("vector only", &[0.8, 0.6, 0.0, 0.0]),
            ("shared", &[1.0, 0.0, 0.0, 0.0]),
        ],
    );
    index(&store, &embedder, &chunks);

    let results = query(&store, &embedder, "shared", 10).expect("query the index");

    assert_eq!(results[0].content, "shared anchor", "the chunk matching both lists wins");
    assert_eq!(results[1].content, "vector only");
    assert!(results[0].score > results[1].score);
    assert!((results[0].score - 2.0 / 61.0).abs() < 1e-12);
    assert!((results[1].score - 1.0 / 62.0).abs() < 1e-12);
}

#[test]
fn query_before_any_embedding_fails_closed() {
    let (_dir, path) = temp_db();
    let store = open_store(&path);
    let embedder = FakeEmbedder::new(4, &[]);

    let error = query(&store, &embedder, "anything", 5).expect_err("empty registry must fail");
    assert!(matches!(error, QueryError::NothingEmbedded { model } if model == MODEL));

    assert!(
        query(&store, &embedder, "anything", 0).expect("zero top_k is a no-op").is_empty(),
        "a zero limit must not resolve the collection"
    );
}

#[test]
fn a_stale_adapter_width_is_rejected() {
    let (_dir, path) = temp_db();
    let store = open_store(&path);
    let chunks = [(1, 1, "payload")];
    seed_file(&store, "docs/width.md", &chunks);
    index(&store, &FakeEmbedder::new(4, &[("payload", &[1.0, 0.0, 0.0, 0.0])]), &chunks);

    let stale = FakeEmbedder::new(3, &[]);
    let error = query(&store, &stale, "payload", 5).expect_err("a width mismatch must fail");

    assert!(matches!(error, QueryError::DimensionMismatch { expected: 4, actual: 3 }));
}

#[test]
fn tied_fused_scores_order_by_chunk_id_and_truncate_deterministically() {
    let (_dir, path) = temp_db();
    let store = open_store(&path);
    // `second anchor` is embedded but has no keyword match; `needle second` is the reverse.
    let tied = [(1, 1, "plain anchor"), (2, 2, "second anchor"), (3, 3, "needle needle first")];
    seed_file(&store, "docs/tie-a.md", &tied);
    seed_file(&store, "docs/tie-b.md", &[(4, 4, "needle second")]);
    store.ensure_embedding_table(4, "cosine").expect("create embeddings_d4");
    store.register_model(MODEL, 4, "cosine").expect("register model");
    for (content, vector) in [
        ("plain anchor", [1.0_f32, 0.0, 0.0, 0.0]),
        ("second anchor", [0.9, 0.435, 0.0, 0.0]),
        ("needle needle first", [0.5, 0.866, 0.0, 0.0]),
    ] {
        store
            .insert_embedding(content_hash(content.as_bytes()).as_bytes(), MODEL, 4, &vector)
            .expect("insert embedding");
    }
    let embedder = FakeEmbedder::new(4, &[("needle", &[1.0, 0.0, 0.0, 0.0])]);

    let results = query(&store, &embedder, "needle", 10).expect("query the index");

    assert_eq!(results.len(), 4);
    assert_eq!(results[0].content, "needle needle first", "both lists' rank-1 chunk");
    assert_eq!(results[1].content, "plain anchor", "the vector-only rank-1 chunk");
    let a_id = content_hash(b"second anchor");
    let b_id = content_hash(b"needle second");
    let (first_tied, second_tied) =
        if a_id.as_bytes() < b_id.as_bytes() { (a_id, b_id) } else { (b_id, a_id) };
    assert_eq!(results[2].chunk_id, first_tied.as_bytes());
    assert_eq!(results[3].chunk_id, second_tied.as_bytes());
    assert!(
        (results[2].score - results[3].score).abs() < f64::EPSILON,
        "the two single-list rank-2 chunks share one fused score"
    );

    let capped = query(&store, &embedder, "needle", 3).expect("query with a smaller limit");
    assert_eq!(capped.len(), 3);
    assert_eq!(capped[2].chunk_id, first_tied.as_bytes(), "ties truncate by chunk id");
    assert_eq!(
        capped,
        query(&store, &embedder, "needle", 3).expect("repeat the query"),
        "tied scores must order deterministically"
    );
}
