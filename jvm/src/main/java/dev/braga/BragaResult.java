// SPDX-License-Identifier: Apache-2.0

package dev.braga;

/** The result code returned by every fallible {@code braga_*} entry point, mirroring {@code
 * braga_result} in {@code braga.h}. */
public enum BragaResult {
    /** The call succeeded. */
    OK(0),
    /** Null pointer, bad UTF-8, bad length, or a null out-parameter. */
    INVALID_ARG(1),
    /** Configuration resolution or validation failed. */
    CONFIG(2),
    /** Key file load failed. */
    KEY(3),
    /** Storage error: wrong key, I/O, or a numeric SQLite code. */
    STORE(4),
    /** Embedder build or provider call failed. */
    EMBED(5),
    /** Nothing is embedded for the configured model. */
    NOT_FOUND(6),
    /** I/O error outside the store. */
    IO(7),
    /** A Rust panic was caught inside the library. */
    PANIC(8),
    /** Allocation or JSON serialization failed. */
    OOM(9);

    private final int code;

    BragaResult(int code) {
        this.code = code;
    }

    /** Returns the frozen ABI discriminant. */
    public int code() {
        return code;
    }

    /** Decodes a raw library code; an unknown value fails closed to {@link #PANIC}. */
    public static BragaResult fromCode(int value) {
        for (BragaResult result : values()) {
            if (result.code == value) {
                return result;
            }
        }
        return PANIC;
    }
}
