// SPDX-License-Identifier: Apache-2.0

package dev.braga;

import com.fasterxml.jackson.core.type.TypeReference;
import com.fasterxml.jackson.databind.ObjectMapper;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.util.List;

/**
 * An open bRAGa index handle: a typed facade over the frozen C ABI (version 1).
 *
 * <p>It consumes the C ABI only and never links the Rust core. A handle is <strong>not
 * thread-safe</strong>: use one {@code Braga} from one thread at a time, as {@code braga.h}
 * requires. A handle is freed exactly once by {@link #close()}; every other method reports {@link
 * BragaResult#INVALID_ARG} after that.
 */
public final class Braga implements AutoCloseable {

    private static final ObjectMapper JSON = new ObjectMapper();

    private long handle;

    private Braga(long handle) {
        this.handle = handle;
    }

    /** Returns the ABI version the loaded library implements; always 1 for this wrapper. */
    public static int abiVersion() {
        return Native.abiVersion();
    }

    /**
     * Resolves the configuration and key, builds the embedder, opens and closes the store once, and
     * returns a live handle. A null path defers to the ABI's environment and defaults ({@code
     * BRAGA_TOML}, {@code BRAGA_CONFIG}, {@code BRAGA_*__*}; {@code BRAGA_KEY_FILE} then {@code
     * ~/.braga/key}).
     */
    public static Braga open(String configPath, String keyPath) throws BragaException {
        long live = Native.open(path(configPath), path(keyPath));
        if (live == 0) {
            throw new BragaException(BragaResult.PANIC, "braga_open reported OK without a handle");
        }
        return new Braga(live);
    }

    /**
     * Writes the default {@code braga.toml} and a fresh key file, refusing to overwrite either
     * existing target. A null path follows the same defaults as {@link #open(String, String)}.
     */
    public static void init(String configPath, String keyPath) throws BragaException {
        Native.init(path(configPath), path(keyPath));
    }

    /** Reports the reconciler state left by the last open or update; a closed handle is ERROR. */
    public BragaStatus status() {
        return handle == 0 ? BragaStatus.ERROR : BragaStatus.fromCode(Native.status(handle));
    }

    /** Reconciles the scan directory and embeds every chunk missing a vector, blocking until the
     * index is queryable. */
    public void update() throws BragaException {
        Native.update(live());
    }

    /**
     * Returns the top {@code topK} hits for {@code text}. An empty index is {@link
     * BragaResult#NOT_FOUND}, never an empty success.
     */
    public List<Hit> query(String text, int topK) throws BragaException {
        long live = live();
        if (text == null || text.isEmpty()) {
            throw new BragaException(BragaResult.INVALID_ARG, "text must be non-empty valid UTF-8");
        }
        if (topK < 0) {
            throw new BragaException(BragaResult.INVALID_ARG, "topK must fit in a uint32");
        }
        byte[] json = Native.query(live, text.getBytes(StandardCharsets.UTF_8), topK);
        return parse(json, new TypeReference<List<Hit>>() {});
    }

    /** Runs the claims verifier and returns its typed report. */
    public DoctorReport doctor() throws BragaException {
        return parse(Native.doctor(live()), new TypeReference<DoctorReport>() {});
    }

    /** Frees the handle. It is idempotent: a second call is a no-op. */
    @Override
    public void close() throws BragaException {
        long closing = handle;
        if (closing == 0) {
            return;
        }
        handle = 0;
        Native.close(closing);
    }

    /** The raw handle, for tests that must exercise the JNI boundary directly. */
    long rawHandle() {
        return handle;
    }

    private long live() throws BragaException {
        if (handle == 0) {
            throw new BragaException(BragaResult.INVALID_ARG, "handle is closed");
        }
        return handle;
    }

    private static byte[] path(String value) {
        return value == null ? null : value.getBytes(StandardCharsets.UTF_8);
    }

    private static <T> T parse(byte[] json, TypeReference<T> type) throws BragaException {
        try {
            return JSON.readValue(json, type);
        } catch (IOException error) {
            throw new BragaException(BragaResult.PANIC, "invalid JSON from the library");
        }
    }
}
