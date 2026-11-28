// SPDX-License-Identifier: Apache-2.0

//! Configuration model for `bRAGa`.
//!
//! Configuration is resolved from up to five layers, lowest precedence first:
//!
//! 1. the built-in defaults ([`Config::default`]), which are also what [`Config::default_toml`]
//!    emits and what the future `braga init` will write,
//! 2. `BRAGA_*__*` leaf overrides over those defaults,
//! 3. the file named by `BRAGA_TOML`,
//! 4. the inline `TOML` document in `BRAGA_CONFIG`,
//! 5. the explicit path passed to [`Config::resolve`].
//!
//! Layers merge per field: a layer only contributes the fields it names, so a `BRAGA_TOML` file
//! that sets only `[embedding]` keeps `scan` and `storage` from the lower layers. Unknown `TOML`
//! keys and unknown or malformed `BRAGA_*` variables are errors; nothing is silently ignored.
//! [`Config::resolve`] documents the environment namespace in full.

mod env;
mod resolve;
mod schema;
#[cfg(test)]
mod tests;

pub use schema::{Config, ConfigError, Embedding, Scan, Storage};
