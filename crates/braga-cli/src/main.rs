// SPDX-License-Identifier: Apache-2.0

//! The `braga` command-line interface: `init`, `scan`, `query`, and `doctor`.
//!
//! Results go to stdout; every telemetry span and event goes to stderr. Telemetry is off unless
//! `--verbose` or a non-empty `BRAGA_LOG` is set.

use std::{path::PathBuf, process::ExitCode};

use clap::{Parser, Subcommand};

mod commands;
mod telemetry;

/// Local-first, encrypted retrieval toolkit for RAG pipelines.
#[derive(Debug, Parser)]
#[command(name = "braga", version, about, long_about = None)]
struct Cli {
    /// Emit telemetry spans and events to stderr.
    #[arg(long, global = true)]
    verbose: bool,

    /// Path of the configuration file.
    #[arg(long, global = true, value_name = "PATH")]
    config: Option<PathBuf>,

    /// Path of the 32-byte database key file.
    #[arg(long, global = true, value_name = "PATH")]
    key_file: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

/// The four `braga` subcommands.
#[derive(Debug, Subcommand)]
enum Command {
    /// Write the default configuration and generate a key file.
    Init,

    /// Reconcile the scan directory into the index and embed new chunks.
    Scan,

    /// Query the indexed chunks.
    Query {
        /// Text to search for.
        text: String,

        /// Maximum number of hits to print.
        #[arg(long, default_value_t = 10)]
        top_k: usize,
    },

    /// Verify the stack's claims and print a JSON report.
    Doctor,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let braga_log = std::env::var("BRAGA_LOG").ok();
    telemetry::init_subscriber(cli.verbose, braga_log.as_deref());

    let outcome = match cli.command {
        Command::Init => commands::init(cli.config.as_deref(), cli.key_file.as_deref()),
        Command::Scan => commands::scan(cli.config.as_deref(), cli.key_file.as_deref()),
        Command::Query { text, top_k } => {
            commands::query(cli.config.as_deref(), cli.key_file.as_deref(), &text, top_k)
        }
        Command::Doctor => commands::doctor(cli.config.as_deref(), cli.key_file.as_deref()),
    };

    match outcome {
        Ok(code) => code,
        Err(error) => {
            eprintln!("braga: {error}");
            ExitCode::FAILURE
        }
    }
}
