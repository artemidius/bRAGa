// SPDX-License-Identifier: AGPL-3.0-or-later

//! Blind-index lifecycle: opt-in default, token lookup, truncation, and rekey rebuild.

use std::{path::Path, sync::Arc};

use braga::config::{Config, Embedding, Scan, Storage};
use braga_chunker::content_hash;
use braga_extra::{ExtraError, ExtraKey, TOKEN_LEN, XChachaEncryptor, build, rekey, search};
use braga_storage::{Key, NewChunk, Store};
use rusqlite::Connection;
use tempfile::TempDir;

fn db_key() -> Key {
    Key::from_bytes([0x5A; 32])
}

fn config(root: &Path) -> Config {
    Config {
        scan: Scan { dir: root.join("docs"), ..Scan::default() },
        embedding: Embedding::default(),
        storage: Storage { path: root.join("index.db") },
    }
}

fn open_extra(path: &Path, master: &ExtraKey) -> Store {
    Store::builder()
        .with_encryptor(Arc::new(XChachaEncryptor::new(master.clone())))
        .open(path, &db_key())
        .expect("open extra store")
}

fn chunk(store: &Store, path: &str, text: &str) {
    let file_hash = content_hash(path.as_bytes());
    let id = content_hash(text.as_bytes());
    store
        .sync_file(
            path,
            file_hash.as_bytes(),
            &[NewChunk {
                id: id.as_bytes(),
                byte_offset: 0,
                line_start: 1,
                line_end: 1,
                content: text,
            }],
        )
        .expect("write chunk");
}

fn seed(config: &Config, master: &ExtraKey) {
    let store = open_extra(&config.storage.path, master);
    chunk(&store, "a.md", "alpha needle text");
    chunk(&store, "b.md", "beta haystack");
}

fn temp_config() -> (TempDir, Config) {
    let temp = TempDir::new().expect("temporary directory");
    let config = config(temp.path());
    (temp, config)
}

#[test]
fn build_and_search_finds_matching_chunks() {
    let (_temp, config) = temp_config();
    let master = ExtraKey::from_bytes([0x11; 32]);
    seed(&config, &master);

    let outcome = build(&config, &db_key(), &master).expect("build the blind index");
    assert_eq!(outcome.chunks, 2);
    assert!(outcome.postings >= 4);

    let hits = search(&config, &db_key(), &master, "needle").expect("search");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].path, "a.md");
    assert_eq!(hits[0].content, "alpha needle text");
    assert_eq!(hits[0].line_start, 1);

    // Matching is case-insensitive and token-based, not a substring match.
    assert_eq!(search(&config, &db_key(), &master, "NEEDLE").expect("search").len(), 1);
    assert_eq!(search(&config, &db_key(), &master, "need").expect("search").len(), 0);
    assert_eq!(search(&config, &db_key(), &master, "haystack").expect("search")[0].path, "b.md");
    assert_eq!(search(&config, &db_key(), &master, "missing").expect("search"), Vec::new());
}

#[test]
fn table_is_opt_in_and_searches_are_single_token() {
    let (_temp, config) = temp_config();
    let master = ExtraKey::from_bytes([0x11; 32]);
    seed(&config, &master);

    assert_eq!(search(&config, &db_key(), &master, "needle").expect("search"), Vec::new());
    assert!(matches!(
        search(&config, &db_key(), &master, "needle text"),
        Err(ExtraError::InvalidData { .. })
    ));
    assert_eq!(search(&config, &db_key(), &master, "  ").expect("empty query"), Vec::new());
}

#[test]
fn postings_are_truncated_hashes_not_tokens() {
    let (_temp, config) = temp_config();
    let master = ExtraKey::from_bytes([0x11; 32]);
    seed(&config, &master);
    build(&config, &db_key(), &master).expect("build the blind index");

    let connection = Connection::open(&config.storage.path).expect("open database");
    connection.key(None::<&str>, db_key().as_bytes(), false).expect("apply database key");
    let tokens: Vec<Vec<u8>> = connection
        .prepare("SELECT token FROM blind_index")
        .expect("prepare")
        .query_map([], |row| row.get(0))
        .expect("query")
        .collect::<rusqlite::Result<_>>()
        .expect("collect");
    assert_ne!(tokens.len(), 0);
    for token in tokens {
        assert_eq!(token.len(), TOKEN_LEN);
        assert!(!token.windows(b"needle".len()).any(|window| window == b"needle"));
    }
}

#[test]
fn rekey_rebuilds_the_index_for_the_new_key() {
    let (_temp, config) = temp_config();
    let old = ExtraKey::from_bytes([0x11; 32]);
    let new = ExtraKey::from_bytes([0x22; 32]);
    seed(&config, &old);
    build(&config, &db_key(), &old).expect("build with the old key");
    assert_eq!(search(&config, &db_key(), &old, "needle").expect("search").len(), 1);

    rekey(&config, &db_key(), &old, &new).expect("rotate the master key");

    assert_eq!(search(&config, &db_key(), &new, "needle").expect("search").len(), 1);
    assert_eq!(search(&config, &db_key(), &old, "needle").expect("search"), Vec::new());
}
