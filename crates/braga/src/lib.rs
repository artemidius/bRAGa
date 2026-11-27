// SPDX-License-Identifier: Apache-2.0

//! Local-first, encrypted retrieval toolkit for RAG pipelines.
//!
//! `bRAGa` builds, maintains, and queries a retrieval index over local files; it never
//! generates text. This crate is under active development and is not audited.
//!
//! [`config`] holds the configuration model and its layered environment resolution. [`scanner`]
//! holds the synchronous reconcile pipeline: it walks the configured directory, hashes each file,
//! re-chunks the changed ones through `braga-chunker`, and replaces their chunks in the encrypted
//! `braga-storage` [`Store`](braga_storage::Store). A background status engine is a later stage.

pub mod config;
pub mod scanner;
