// SPDX-License-Identifier: AGPL-3.0-or-later

//! AEAD round trips, wrong-key negatives, and the update/rekey lifecycle over a real database.

use std::{fs, path::Path, sync::Arc};

use braga::{
    config::{Config, Embedding, Scan, Storage},
    seam::{Aad, ChunkEncryptor as _, RecordKind, SeamError},
};
use braga_embedder::{EmbedError, Embedder};
use braga_extra::{ExtraKey, XChachaEncryptor, rekey, status, update};
use braga_storage::{Key, Store, StoreError};
use tempfile::TempDir;

const MODEL: &str = "fake";
const WIDTH: usize = 4;
const DIM: u32 = 4;

fn chunk_aad(chunk_id: &[u8]) -> Aad {
    Aad {
        collection: "default".to_owned(),
        model: None,
        chunk_id: chunk_id.to_vec(),
        kind: RecordKind::Chunk,
    }
}

fn embedding_aad(model: &str, chunk_id: &[u8]) -> Aad {
    Aad {
        collection: "default".to_owned(),
        model: Some(model.to_owned()),
        chunk_id: chunk_id.to_vec(),
        kind: RecordKind::Embedding,
    }
}

fn encryptor(byte: u8) -> XChachaEncryptor {
    XChachaEncryptor::new(ExtraKey::from_bytes([byte; 32]))
}

fn db_key() -> Key {
    Key::from_bytes([0x5A; 32])
}

fn open_extra(path: &Path, master: &ExtraKey) -> Store {
    Store::builder()
        .with_encryptor(Arc::new(XChachaEncryptor::new(master.clone())))
        .open(path, &db_key())
        .expect("open extra store")
}

struct FakeEmbedder {
    model: String,
}

impl FakeEmbedder {
    fn new() -> Self {
        Self { model: MODEL.to_owned() }
    }
}

impl Embedder for FakeEmbedder {
    fn embed(&self, inputs: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError> {
        Ok(inputs.iter().map(|text| vector(text)).collect())
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn dim(&self) -> Option<usize> {
        Some(WIDTH)
    }
}

/// A deterministic, non-zero vector derived from the text bytes.
fn vector(text: &str) -> Vec<f32> {
    let bytes = text.as_bytes();
    let fallback = bytes.first().copied().unwrap_or(0);
    (0..WIDTH)
        .map(|index| {
            let byte = bytes.get(index).copied().unwrap_or(fallback);
            f32::from(byte) / 255.0
        })
        .collect()
}

fn config(root: &Path) -> Config {
    Config {
        scan: Scan { dir: root.join("docs"), ..Scan::default() },
        embedding: Embedding::default(),
        storage: Storage { path: root.join("index.db") },
    }
}

#[test]
fn chunk_and_embedding_records_round_trip() {
    let chunk_sealed =
        encryptor(0x11).encrypt(&chunk_aad(&[0x21; 32]), b"chunk text").expect("seal chunk");
    let chunk_open =
        encryptor(0x11).decrypt(&chunk_aad(&[0x21; 32]), &chunk_sealed).expect("open chunk");
    assert_eq!(chunk_open, b"chunk text");

    let embedding_sealed = encryptor(0x11)
        .encrypt(&embedding_aad(MODEL, &[0x22; 32]), b"vector bytes")
        .expect("seal embedding");
    let embedding_open = encryptor(0x11)
        .decrypt(&embedding_aad(MODEL, &[0x22; 32]), &embedding_sealed)
        .expect("open embedding");
    assert_eq!(embedding_open, b"vector bytes");
}

#[test]
fn a_wrong_key_a_tampered_ciphertext_and_a_moved_aad_all_fail_closed() {
    let sealed = encryptor(0x11).encrypt(&chunk_aad(&[0x31; 32]), b"secret").expect("seal");

    assert!(matches!(
        encryptor(0x12).decrypt(&chunk_aad(&[0x31; 32]), &sealed),
        Err(SeamError::Decrypt)
    ));

    let mut tampered = sealed.clone();
    *tampered.last_mut().expect("ciphertext is not empty") ^= 0x01;
    assert!(matches!(
        encryptor(0x11).decrypt(&chunk_aad(&[0x31; 32]), &tampered),
        Err(SeamError::Decrypt)
    ));

    assert!(matches!(
        encryptor(0x11).decrypt(&chunk_aad(&[0x32; 32]), &sealed),
        Err(SeamError::Decrypt)
    ));
    assert!(matches!(
        encryptor(0x11).decrypt(&embedding_aad(MODEL, &[0x31; 32]), &sealed),
        Err(SeamError::Decrypt)
    ));
}

#[test]
fn encryption_uses_a_fresh_nonce_per_record() {
    let first = encryptor(0x11).encrypt(&chunk_aad(&[0x41; 32]), b"same").expect("first");
    let second = encryptor(0x11).encrypt(&chunk_aad(&[0x41; 32]), b"same").expect("second");
    assert_ne!(first, second, "two seals of one record must not share a nonce");
}

fn write_doc(root: &Path, name: &str, body: &str) {
    fs::write(root.join(name), body).expect("write fixture");
}

#[test]
fn update_rekey_and_status_preserve_plaintext() {
    let temp = TempDir::new().expect("temporary directory");
    let docs = temp.path().join("docs");
    fs::create_dir(&docs).expect("create scan directory");
    write_doc(&docs, "a.md", "# Alpha\n\nalpha text\n");
    write_doc(&docs, "b.md", "beta text\nneedle text\n");
    let config = config(temp.path());
    let master = ExtraKey::from_bytes([0x11; 32]);
    let embedder = FakeEmbedder::new();

    let updated = update(&config, &db_key(), &master, &embedder).expect("update the index");
    assert!(updated.scan.added >= 2, "both files must be indexed");
    assert!(updated.embedding.embedded >= 1, "chunks must be embedded");

    let report = status(&config, &db_key()).expect("read status");
    assert!(report.encrypted_embeddings);
    assert!(report.embeddings > 0);
    assert_eq!(report.chunks, report.embeddings);
    assert_eq!(report.files, 2);

    let before = {
        let store = open_extra(&config.storage.path, &master);
        let chunks = store.all_chunks().expect("read decrypted chunks");
        let embeddings = store.list_embeddings(MODEL, DIM).expect("read decrypted embeddings");
        assert!(chunks.iter().any(|(_, text)| text.contains("alpha text")));
        assert!(chunks.iter().any(|(_, text)| text.contains("needle text")));
        (chunks, embeddings)
    };

    let new_master = ExtraKey::from_bytes([0x22; 32]);
    let rekeyed = rekey(&config, &db_key(), &master, &new_master).expect("rotate the master key");
    assert_eq!(rekeyed.chunks, before.0.len());
    assert_eq!(rekeyed.embeddings, before.1.len());

    let store = open_extra(&config.storage.path, &new_master);
    assert_eq!(store.all_chunks().expect("read with the new key"), before.0);
    assert_eq!(
        store.list_embeddings(MODEL, DIM).expect("read embeddings with the new key"),
        before.1
    );

    let old = open_extra(&config.storage.path, &master);
    assert!(matches!(old.all_chunks(), Err(StoreError::Seam(SeamError::Decrypt))));
}
