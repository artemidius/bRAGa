// SPDX-License-Identifier: Apache-2.0

package braga

/*
// ${SRCDIR} is this package directory (go/), so the C header and the prebuilt cdylib live one
// level up in the repository root. Building the library first is a documented prerequisite:
// cargo build --release -p braga-ffi
#cgo CFLAGS: -I${SRCDIR}/../crates/braga-ffi/include
#cgo LDFLAGS: -L${SRCDIR}/../target/release -lbraga_ffi -Wl,-rpath,${SRCDIR}/../target/release
#include <stdlib.h>
#include "braga.h"
*/
import "C"

import (
	"fmt"
	"runtime"
	"unsafe"
)

// abiVersion is the BRAGA_ABI_VERSION this wrapper is written against.
const abiVersion uint32 = 1

// handle is an opaque braga_handle pointer. The typed layer keeps its C surface hidden.
type handle unsafe.Pointer

func init() {
	if got := _braga_abi_version(); got != abiVersion {
		panic(fmt.Sprintf(
			"braga: library ABI version %d does not match the wrapper's %d; refusing to load",
			got, abiVersion,
		))
	}
}

// ptr converts the opaque handle to the C pointer type used by the ABI.
func ptr(h handle) *C.braga_handle { return (*C.braga_handle)(unsafe.Pointer(h)) }

// cArg encodes a Go string as the bytes-plus-length the ABI expects. An empty string is NULL/0,
// which defers to the environment and the built-in defaults for the path arguments.
func cArg(value string) (*C.uint8_t, C.size_t, func()) {
	if value == "" {
		return nil, 0, func() {}
	}
	raw := C.CBytes([]byte(value))
	return (*C.uint8_t)(raw), C.size_t(len(value)), func() { C.free(raw) }
}

// readLastError copies the thread-local message. It is only called with the calling goroutine
// locked to its OS thread, on the same thread as the call whose error it reports.
func readLastError() string {
	return C.GoString(C.braga_last_error(nil))
}

func _braga_abi_version() uint32 { return uint32(C.braga_abi_version()) }

// _braga_status is infallible and never touches the error buffer, so it needs no thread lock.
func _braga_status(h handle) Status {
	return Status(C.braga_status(ptr(h)))
}

func _braga_open(configPath, keyPath string) (Result, handle, string) {
	runtime.LockOSThread()
	defer runtime.UnlockOSThread()

	configPtr, configLen, freeConfig := cArg(configPath)
	defer freeConfig()
	keyPtr, keyLen, freeKey := cArg(keyPath)
	defer freeKey()

	var out *C.braga_handle
	code := Result(C.braga_open(configPtr, configLen, keyPtr, keyLen, &out))
	return code, handle(unsafe.Pointer(out)), readLastError()
}

func _braga_close(h handle) (Result, string) {
	runtime.LockOSThread()
	defer runtime.UnlockOSThread()

	code := Result(C.braga_close(ptr(h)))
	return code, readLastError()
}

func _braga_update(h handle) (Result, string) {
	runtime.LockOSThread()
	defer runtime.UnlockOSThread()

	// The reserved key slot stays NULL in base mode; braga-extra makes it live without an ABI break.
	code := Result(C.braga_update(ptr(h), nil, 0))
	return code, readLastError()
}

// _braga_query passes text as an explicit byte span so a nil slice (NULL/0) and a non-nil empty
// slice stay distinguishable; the ABI rejects both. The reserved key slot is NULL in base mode.
func _braga_query(h handle, text []byte, topK uint32) (Result, string, string) {
	runtime.LockOSThread()
	defer runtime.UnlockOSThread()

	var textPtr *C.uint8_t
	var textLen C.size_t
	if text != nil {
		// The extra NUL terminator is convention only; the explicit length is authoritative, so
		// any embedded NUL in text is preserved.
		payload := append(append([]byte(nil), text...), 0)
		raw := C.CBytes(payload)
		defer C.free(raw)
		textPtr = (*C.uint8_t)(raw)
		textLen = C.size_t(len(text))
	}

	var out *C.char
	var outLen C.size_t
	code := Result(C.braga_query(
		ptr(h), textPtr, textLen, C.uint32_t(topK), nil, 0, &out, &outLen,
	))

	var data string
	if code == ResultOK && out != nil {
		data = string(C.GoBytes(unsafe.Pointer(out), C.int(outLen)))
		C.braga_string_free(out)
	}
	return code, data, readLastError()
}

func _braga_doctor(h handle) (Result, string, string) {
	runtime.LockOSThread()
	defer runtime.UnlockOSThread()

	var out *C.char
	var outLen C.size_t
	code := Result(C.braga_doctor(ptr(h), &out, &outLen))

	var data string
	if code == ResultOK && out != nil {
		data = string(C.GoBytes(unsafe.Pointer(out), C.int(outLen)))
		C.braga_string_free(out)
	}
	return code, data, readLastError()
}

func _braga_init(configPath, keyPath string) (Result, string) {
	runtime.LockOSThread()
	defer runtime.UnlockOSThread()

	configPtr, configLen, freeConfig := cArg(configPath)
	defer freeConfig()
	keyPtr, keyLen, freeKey := cArg(keyPath)
	defer freeKey()

	code := Result(C.braga_init(configPtr, configLen, keyPtr, keyLen))
	return code, readLastError()
}
