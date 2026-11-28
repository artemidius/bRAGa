// SPDX-License-Identifier: Apache-2.0

// Package braga is an idiomatic Go binding over the frozen bRAGa C ABI (version 1).
//
// It consumes the C ABI only; it never links the Rust core. The library must be built before the
// package is linked:
//
//	cargo build --release -p braga-ffi
//
// The cgo directives in ffi.go then link the resulting cdylib from target/release.
package braga

import "unicode/utf8"

// Braga is an open index handle.
//
// A Braga is NOT safe for concurrent use. The underlying braga_handle is single-threaded by
// contract, so use one Braga from one goroutine at a time; do not share it and do not call its
// methods concurrently. The binding locks the OS thread around every fallible call only to read
// the thread-local error buffer correctly, which does not make the handle thread-safe.
//
// A handle is freed exactly once by Close; every other method reports ResultInvalidArg after that.
type Braga struct {
	handle handle
}

// AbiVersion returns the ABI version the loaded library implements.
func AbiVersion() uint32 { return _braga_abi_version() }

// Open resolves the configuration and key, builds the embedder, opens and closes the store once,
// and returns a live handle. An empty path defers to the ABI's environment and defaults
// (BRAGA_TOML, BRAGA_CONFIG, BRAGA_*__*; BRAGA_KEY_FILE then ~/.braga/key).
func Open(configPath, keyPath string) (*Braga, error) {
	code, live, message := _braga_open(configPath, keyPath)
	if code != ResultOK {
		return nil, &BragaError{Code: code, Message: message}
	}
	if live == nil {
		return nil, &BragaError{Code: ResultPanic, Message: "braga_open reported OK without a handle"}
	}
	return &Braga{handle: live}, nil
}

// Init writes the default braga.toml and a fresh key file, refusing to overwrite either existing
// target. An empty path follows the same defaults as Open.
func Init(configPath, keyPath string) error {
	code, message := _braga_init(configPath, keyPath)
	if code != ResultOK {
		return &BragaError{Code: code, Message: message}
	}
	return nil
}

// live returns the handle or the use-after-close error.
func (b *Braga) live() (handle, error) {
	if b == nil || b.handle == nil {
		return nil, &BragaError{Code: ResultInvalidArg, Message: "handle is closed"}
	}
	return b.handle, nil
}

// Status reports the reconciler state left by the last Open or Update. A closed handle fails
// closed to StatusError, matching a NULL handle in the ABI.
func (b *Braga) Status() Status {
	if b == nil || b.handle == nil {
		return StatusError
	}
	return statusFromCode(_braga_status(b.handle))
}

// Update reconciles the scan directory and embeds every chunk missing a vector, blocking until the
// index is queryable. The reserved key slot is NULL in base mode.
func (b *Braga) Update() error {
	live, err := b.live()
	if err != nil {
		return err
	}
	code, message := _braga_update(live)
	if code != ResultOK {
		return &BragaError{Code: code, Message: message}
	}
	return nil
}

// Query returns the top topK hits for text. An empty index is ResultNotFound, never an empty
// success. The reserved key slot of braga_query stays NULL in base mode.
func (b *Braga) Query(text string, topK int) ([]Hit, error) {
	live, err := b.live()
	if err != nil {
		return nil, err
	}
	if text == "" || !utf8.ValidString(text) {
		return nil, &BragaError{Code: ResultInvalidArg, Message: "text must be non-empty valid UTF-8"}
	}
	if topK < 0 || topK > 0xFFFFFFFF {
		return nil, &BragaError{Code: ResultInvalidArg, Message: "topK must fit in a uint32"}
	}
	code, data, message := _braga_query(live, []byte(text), uint32(topK))
	if code != ResultOK {
		return nil, &BragaError{Code: code, Message: message}
	}
	hits, err := parseHits([]byte(data))
	if err != nil {
		return nil, err
	}
	return hits, nil
}

// Doctor runs the claims verifier and returns its typed report.
func (b *Braga) Doctor() (DoctorReport, error) {
	live, err := b.live()
	if err != nil {
		return DoctorReport{}, err
	}
	code, data, message := _braga_doctor(live)
	if code != ResultOK {
		return DoctorReport{}, &BragaError{Code: code, Message: message}
	}
	return parseDoctor([]byte(data))
}

// Close frees the handle. It is idempotent: a second call is a no-op.
func (b *Braga) Close() error {
	if b == nil || b.handle == nil {
		return nil
	}
	live := b.handle
	b.handle = nil
	code, message := _braga_close(live)
	if code != ResultOK {
		return &BragaError{Code: code, Message: message}
	}
	return nil
}
