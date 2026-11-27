// SPDX-License-Identifier: Apache-2.0

//! Local-first, encrypted retrieval toolkit for RAG pipelines.
//!
//! `bRAGa` builds, maintains, and queries a retrieval index over local files; it never
//! generates text. This crate is under active development and is not audited.
//!
//! Stage 2 provides the configuration model and its layered environment resolution; indexing and
//! storage land in later stages.

pub mod config;
