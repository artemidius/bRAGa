// SPDX-License-Identifier: Apache-2.0

//! Local-first, encrypted retrieval toolkit for RAG pipelines.
//!
//! `bRAGa` builds, maintains, and queries a retrieval index over local files; it never
//! generates text. This crate is under active development and is not audited.
//!
//! [`config`] holds the configuration model and its layered environment resolution. [`scanner`]
//! holds the synchronous reconcile pipeline: it walks the configured directory, hashes each file,
//! re-chunks the changed ones through `braga-chunker`, and replaces their chunks in the encrypted
//! `braga-storage` [`Store`](braga_storage::Store). [`status`] runs that pipeline on one background
//! worker and reports its state through a lock-free [`DataStatus`](status::DataStatus).
//! [`embedding`] closes the write half of the retrieval pipeline: it cache-filters chunks against
//! `model_registry` and the per-dimension `vec0` table, embeds the missing ones through a
//! [`braga_embedder::Embedder`], and stores each vector with
//! [`Store::insert_embedding`](braga_storage::Store::insert_embedding).

pub mod config;
pub mod embedding;
pub mod scanner;
pub mod status;
