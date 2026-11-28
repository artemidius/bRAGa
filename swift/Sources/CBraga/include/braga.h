/*
 * SPDX-License-Identifier: Apache-2.0
 *
 * bRAGa C ABI, version 1 (frozen).
 *
 * This header is hand-written and is the contract. A wrapper MUST compare
 * `braga_abi_version()` with the `BRAGA_ABI_VERSION` it was compiled against and refuse to load
 * on a mismatch. After this version, a breaking change is a new `BRAGA_ABI_VERSION` and a new
 * symbol surface, never a silent edit to an existing one.
 *
 * Threading
 * ---------
 * A `braga_handle` is NOT thread-safe: use it from one thread at a time. A handle is created by
 * `braga_open` and destroyed by `braga_close`. The error buffer behind `braga_last_error` is
 * per-thread, so a message can only be read on the thread that produced it.
 *
 * Strings
 * -------
 * Every string crosses as `const uint8_t *` plus an explicit `size_t` length. The length is
 * authoritative (chunk content may contain NUL bytes), and paths and text MUST be valid UTF-8:
 * the library returns BRAGA_INVALID_ARG otherwise.
 *
 * Memory ownership
 * ----------------
 * - Borrowed inputs (`config_path`, `key_path`, `text`, `key`): caller-owned, valid and unaliased
 *   for the duration of the call.
 * - `braga_handle`: library-owned; freed by `braga_close`.
 * - JSON output (`braga_query`, `braga_doctor`): library-allocated and NUL-terminated; the caller
 *   frees it with `braga_string_free`. Both out-parameters are set to NULL/0 on error.
 * - `braga_last_error`: thread-local and library-owned; valid until the next FFI call on that
 *   thread. Never free it.
 *
 * Scope in v1
 * ------------
 * There is no background or periodic scan. `braga_update` is explicit and blocking, mirroring the
 * CLI's `scan`; a wrapper that wants periodic scanning drives `braga_update` on its own timer.
 * `braga_status` reflects the outcome of the last `braga_open` or `braga_update`.
 */

#ifndef BRAGA_H
#define BRAGA_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* The ABI version implemented by the library; `braga_abi_version()` returns it. */
#define BRAGA_ABI_VERSION 1

/* Opaque handle returned by `braga_open` and released by `braga_close`. */
typedef struct braga_handle braga_handle;

/* Result code returned by every fallible entry point. */
typedef enum braga_result {
    BRAGA_OK          = 0, /* the call succeeded */
    BRAGA_INVALID_ARG = 1, /* null pointer, bad UTF-8, bad length, or null out-parameter */
    BRAGA_CONFIG      = 2, /* configuration resolution or validation failed */
    BRAGA_KEY         = 3, /* key file load failed */
    BRAGA_STORE       = 4, /* storage error: wrong key, I/O, or numeric SQLite code */
    BRAGA_EMBED       = 5, /* embedder build or provider call failed */
    BRAGA_NOT_FOUND   = 6, /* nothing embedded for the configured model */
    BRAGA_IO          = 7, /* I/O error outside the store */
    BRAGA_PANIC       = 8, /* a Rust panic was caught inside the library */
    BRAGA_OOM         = 9  /* allocation or JSON serialization failed */
} braga_result;

/*
 * Reconciler state reported by `braga_status`. The type is `braga_status_code`, not
 * `braga_status`, because C and C++ put typedefs and functions in one ordinary namespace and
 * `braga_status` is the function's name. The constants below are the stable part of the contract.
 */
typedef enum braga_status_code {
    BRAGA_STATUS_UP_TO_DATE = 0, /* index matches the scan directory; no update in flight */
    BRAGA_STATUS_CHECKING   = 1, /* update in flight, nothing written yet */
    BRAGA_STATUS_UPDATING   = 2, /* update in flight and writing/embedding */
    BRAGA_STATUS_ERROR      = 3  /* the last open or update failed */
} braga_status_code;

/* Returns BRAGA_ABI_VERSION. A wrapper must check it at load. */
uint32_t braga_abi_version(void);

/*
 * Resolves the configuration and key, builds the embedder, opens and closes the store once (so a
 * wrong key fails here), and returns a handle in `*out_handle`.
 *
 * `config_path` NULL/0 defers to BRAGA_TOML, BRAGA_CONFIG, BRAGA_*__* and the built-in defaults;
 * `key_path` NULL/0 defers to BRAGA_KEY_FILE and then `~/.braga/key`.
 *
 * On success `*out_handle` is a new handle the caller must free exactly once with `braga_close`.
 * On error `*out_handle` is NULL.
 */
braga_result braga_open(const uint8_t *config_path, size_t config_path_len,
                        const uint8_t *key_path, size_t key_path_len,
                        braga_handle **out_handle);

/* Frees a handle. Passing the same handle twice, or using it afterwards, is undefined. */
braga_result braga_close(braga_handle *handle);

/* Reports the last status. Never fails otherwise; a NULL handle reports BRAGA_STATUS_ERROR. */
braga_status_code braga_status(const braga_handle *handle);

/*
 * Returns the sanitized message of the most recent failure on the calling thread: no key
 * material, no file content, no query text. The message is empty when the most recent fallible
 * call succeeded. The returned pointer is library-owned and must not be freed. `handle` is accepted
 * for interface uniformity and is never dereferenced.
 */
const char *braga_last_error(const braga_handle *handle);

/*
 * Reconciles the scan directory and embeds every chunk missing a vector. Blocks until the index is
 * queryable. Sets the status to CHECKING, then UPDATING, then UP_TO_DATE or ERROR.
 *
 * `key`/`key_len` are reserved: NULL/0 in base mode, and a non-NULL value is ignored. They exist so
 * `braga-extra` can export this same header and make the slot live without an ABI break.
 */
braga_result braga_update(braga_handle *handle,
                          const uint8_t *key, size_t key_len);

/*
 * Queries the index and returns the top `top_k` hits as a JSON array of
 * {"chunk_id","path","line_start","line_end","content","score"} objects, where `chunk_id` is the
 * lowercase hex of the 32-byte SHA-256 digest. `top_k` 0 returns "[]". `out_json_len` is the
 * authoritative byte length, excluding the NUL terminator. The caller frees `*out_json` with
 * `braga_string_free`. On error both out-parameters are NULL/0.
 *
 * `key`/`key_len` are reserved exactly as in `braga_update`.
 */
braga_result braga_query(braga_handle *handle,
                         const uint8_t *text, size_t text_len,
                         uint32_t top_k,
                         const uint8_t *key, size_t key_len,
                         char **out_json, size_t *out_json_len);

/*
 * Runs the claims verifier and returns its pretty-printed JSON report (`ok` plus the ordered
 * `checks` array). The caller frees `*out_json` with `braga_string_free`. On error both
 * out-parameters are NULL/0.
 */
braga_result braga_doctor(const braga_handle *handle,
                          char **out_json, size_t *out_json_len);

/*
 * Writes the default `braga.toml` and a fresh key file. Both writes are create-new: an existing
 * target fails instead of being overwritten. A NULL config path writes `braga.toml` in the current
 * directory; a NULL key path follows BRAGA_KEY_FILE and then `~/.braga/key`.
 */
braga_result braga_init(const uint8_t *config_path, size_t config_path_len,
                        const uint8_t *key_path, size_t key_path_len);

/* Frees an output buffer returned by `braga_query` or `braga_doctor`. NULL is a no-op. */
void braga_string_free(char *ptr);

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* BRAGA_H */
