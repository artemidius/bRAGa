// SPDX-License-Identifier: Apache-2.0

//! The background status engine: one worker thread that keeps the index reconciled.
//!
//! [`ScanEngine::start`] builds a [`Scanner`] and moves it onto a single worker thread, because
//! the encrypted [`Store`] it owns wraps a `rusqlite::Connection` that is `Send` but not `Sync`.
//! The worker reconciles once on start, then only on a manual [`ScanEngine::update`] and/or a
//! periodic tick, depending on `scan.auto_scan`. [`ScanEngine::status`] reads a lock-free atomic,
//! so polling can never be delayed by an in-flight reconcile.

use std::{
    io,
    sync::{
        Arc, Condvar, Mutex, PoisonError,
        atomic::{AtomicBool, AtomicU8, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, Sender},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use braga_storage::Store;

use crate::{
    config::{Config, ConfigError, Scan},
    scanner::{ScanError, Scanner},
};

/// The reconciler's current state.
///
/// `status()` returns this from an [`AtomicU8`], with no lock, so polling is wait-free and cannot
/// be delayed by an in-flight reconcile. [`Checking`](Self::Checking) and
/// [`Updating`](Self::Updating) are best-effort observability hints: because one
/// [`Scanner::reconcile`](crate::scanner::Scanner::reconcile) both discovers and applies changes, a
/// fast reconcile may move `Checking → UpToDate` before a poll observes the intermediate phases.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataStatus {
    /// The index matches the scan directory and no reconcile is in flight.
    UpToDate = 0,
    /// A reconcile is in flight and has not written to the index yet.
    Checking = 1,
    /// The in-flight reconcile wrote at least one file (add, modify, or delete).
    Updating = 2,
    /// The most recent reconcile failed; its cause is returned by [`ScanEngine::update`].
    Error = 3,
}

impl DataStatus {
    /// Decodes a raw `u8`; any value outside `0..=3` is treated as `Error` (fail closed).
    fn from_u8(value: u8) -> Self {
        match value {
            0 => Self::UpToDate,
            1 => Self::Checking,
            2 => Self::Updating,
            _ => Self::Error,
        }
    }
}

/// Runs one background worker that keeps the encrypted index in line with the scan directory.
///
/// A single worker owns the [`Scanner`] (and therefore the [`Store`]): `Connection` is `Send` but
/// `!Sync`, so single-threaded ownership is forced, not chosen. `status()` reads an atomic;
/// `update()` enqueues a tick and blocks until the resulting reconcile completes; `stop()` shuts the
/// worker down.
///
/// The engine is `Send + Sync`: the shared state lives in `Arc`s so the worker holds clones while
/// the owner polls `status()` (typically from another thread via `Arc<ScanEngine>`).
#[derive(Debug)]
pub struct ScanEngine {
    status: Arc<AtomicU8>,
    cancel: Arc<AtomicBool>,
    tick: Sender<()>,
    completion: Arc<(Mutex<Completion>, Condvar)>,
    worker: Option<JoinHandle<()>>,
}

/// Completion state shared with the worker; guarded by the mutex, waited on by the `Condvar`.
///
/// It never guards the index or the status — only the two 64-bit generation counters and the last
/// error. `generation` is bumped when a reconcile starts, `completed` when it ends, so a waiter
/// can tell a finished reconcile from one that merely started after it sampled.
#[derive(Debug, Default)]
struct Completion {
    generation: u64,
    completed: u64,
    last_error: Option<ScanError>,
}

impl ScanEngine {
    /// Validates the scan config, builds the [`Scanner`], spawns the worker, and runs the on-start
    /// reconcile in the background. Returns immediately; `status()` reports
    /// [`Checking`](DataStatus::Checking) until the initial reconcile settles.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::InvalidScanInterval`] when `scan.auto_scan` is `true` and
    /// `scan.interval_secs` is `0`.
    ///
    /// # Panics
    ///
    /// Panics if the operating system cannot spawn the worker thread.
    pub fn start(config: Config, store: Store) -> Result<Self, ConfigError> {
        let Scan { dir, auto_scan, interval_secs } = config.scan;
        if auto_scan && interval_secs == 0 {
            return Err(ConfigError::InvalidScanInterval);
        }

        let status = Arc::new(AtomicU8::new(DataStatus::Checking as u8)); // never a false UpToDate
        let cancel = Arc::new(AtomicBool::new(false));
        let (tick, rx) = mpsc::channel::<()>();
        let completion = Arc::new((Mutex::new(Completion::default()), Condvar::new()));

        let worker = thread::spawn({
            let status = Arc::clone(&status);
            let cancel = Arc::clone(&cancel);
            let completion = Arc::clone(&completion);
            move || {
                worker_loop(
                    Scanner::new(dir, store),
                    status,
                    cancel,
                    rx,
                    completion,
                    auto_scan,
                    Duration::from_secs(interval_secs),
                );
            }
        });

        Ok(Self { status, cancel, tick, completion, worker: Some(worker) })
    }

    /// The worker's current state. Lock-free atomic load; never blocks.
    #[must_use]
    pub fn status(&self) -> DataStatus {
        DataStatus::from_u8(self.status.load(Ordering::Relaxed))
    }

    /// Triggers one reconcile and blocks until it completes, returning the outcome of that
    /// reconcile.
    ///
    /// On failure the returned [`ScanError`] is the exact error the worker recorded, and
    /// `status()` stays [`Error`](DataStatus::Error) (the atomic is only cleared by the *next*
    /// reconcile).
    ///
    /// # Errors
    ///
    /// Returns the [`ScanError`] the worker produced, or a synthesized [`ScanError::Io`]
    /// (`"scan worker stopped"`) if the worker has already exited (a bug, not a normal state —
    /// `stop(self)` consumes the engine, so the sender is alive for every call here).
    pub fn update(&self) -> Result<(), ScanError> {
        let guard = self.completion.0.lock().unwrap_or_else(PoisonError::into_inner);
        let generation = guard.generation; // sample BEFORE send
        drop(guard);

        self.tick.send(()).map_err(|_| ScanError::Io(io::Error::other("scan worker stopped")))?;

        let mut guard = self.completion.0.lock().unwrap_or_else(PoisonError::into_inner);
        // A reconcile that was already in flight when the generation was sampled completes with
        // `completed == generation`, so it cannot satisfy this wait; only a reconcile that started
        // after the sample can.
        while guard.completed <= generation {
            guard = self.completion.1.wait(guard).unwrap_or_else(PoisonError::into_inner);
        }
        match guard.last_error.take() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// Shuts the worker down: sets the cancel flag, drops the tick sender, and joins the thread.
    ///
    /// Consumes `self`, so `status()`/`update()` are unavailable afterwards. Blocks until an
    /// in-flight `reconcile()` returns (reconcile is monolithic and has no cancellation hook).
    pub fn stop(mut self) {
        self.cancel.store(true, Ordering::Release);
        drop(self.tick); // receiver sees Disconnected → worker exits the loop
        if let Some(handle) = self.worker.take() {
            let _ = handle.join(); // ignore a panicked worker; we are shutting down
        }
    }
}

/// The worker's only job: the on-start reconcile, then one reconcile per trigger.
#[expect(
    clippy::needless_pass_by_value,
    reason = "the worker thread takes ownership of the scanner and the shared state it uses"
)]
fn worker_loop(
    scanner: Scanner,
    status: Arc<AtomicU8>,
    cancel: Arc<AtomicBool>,
    rx: Receiver<()>,
    completion: Arc<(Mutex<Completion>, Condvar)>,
    auto_scan: bool,
    interval: Duration,
) {
    run_once(&scanner, &status, &completion); // on-start reconcile

    loop {
        let triggered = if auto_scan {
            match rx.recv_timeout(interval) {
                Ok(()) | Err(RecvTimeoutError::Timeout) => true, // manual tick | periodic
                Err(RecvTimeoutError::Disconnected) => false,    // stop() dropped the sender
            }
        } else {
            match rx.recv() {
                // manual-only
                Ok(()) => true,
                Err(_) => false, // Disconnected only
            }
        };

        if !triggered || cancel.load(Ordering::Acquire) {
            break;
        }
        run_once(&scanner, &status, &completion);
    }
}

/// Runs one reconcile and publishes its status and completion state to the waiting owner.
fn run_once(scanner: &Scanner, status: &AtomicU8, completion: &(Mutex<Completion>, Condvar)) {
    let (lock, cvar) = (&completion.0, &completion.1);

    status.store(DataStatus::Checking as u8, Ordering::Relaxed);
    lock.lock().unwrap_or_else(PoisonError::into_inner).generation += 1; // bump at START

    let last_error = match scanner.reconcile() {
        Ok(outcome) => {
            if outcome.added + outcome.modified + outcome.deleted > 0 {
                status.store(DataStatus::Updating as u8, Ordering::Relaxed);
            }
            status.store(DataStatus::UpToDate as u8, Ordering::Relaxed);
            None
        }
        Err(error) => {
            status.store(DataStatus::Error as u8, Ordering::Relaxed);
            Some(error)
        }
    };

    let mut guard = lock.lock().unwrap_or_else(PoisonError::into_inner);
    guard.last_error = last_error;
    guard.completed = guard.generation;
    drop(guard);
    cvar.notify_all();
}
