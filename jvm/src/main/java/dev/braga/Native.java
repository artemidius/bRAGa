// SPDX-License-Identifier: Apache-2.0

package dev.braga;

import java.io.IOException;
import java.io.InputStream;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.attribute.PosixFilePermissions;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.util.HexFormat;
import java.util.Locale;

/**
 * The JNI surface over the frozen bRAGa C ABI (version 1).
 *
 * <p>This class extracts the shim and the Rust cdylib from the jar into one private temporary
 * directory and loads the shim; the shim's rpath discovers the cdylib next to it. Every native
 * method throws {@link BragaException} on a non-OK {@code braga_*} result.
 *
 * <p>Strings cross as {@code byte[]} (UTF-8), never as JNI strings: {@code GetStringUTFChars}
 * produces modified UTF-8, which is not the valid UTF-8 the ABI requires. Only {@link
 * #lastError(long)} returns a {@code String}, because the ABI guarantees sanitized ASCII there.
 */
final class Native {

    /** The BRAGA_ABI_VERSION this wrapper is compiled against. */
    static final int ABI_VERSION = 1;

    private static final String DIRECTORY = resourceDirectory();
    private static final String EXTENSION = DIRECTORY.startsWith("macos") ? "dylib" : "so";
    private static final Path SHIM;

    static {
        try {
            SHIM = extractNative();
        } catch (IOException | RuntimeException error) {
            throw new ExceptionInInitializerError(error);
        }
        System.load(SHIM.toAbsolutePath().toString());
        int version = abiVersion();
        if (version != ABI_VERSION) {
            throw new ExceptionInInitializerError(new UnsatisfiedLinkError(
                    "braga: library ABI version " + version + " does not match the wrapper's "
                            + ABI_VERSION + "; refusing to load"));
        }
    }

    private Native() {}

    static native int abiVersion();

    static native long open(byte[] configPath, byte[] keyPath) throws BragaException;

    static native int close(long handle) throws BragaException;

    static native int status(long handle);

    static native String lastError(long handle);

    static native int update(long handle) throws BragaException;

    static native byte[] query(long handle, byte[] text, int topK) throws BragaException;

    static native byte[] doctor(long handle) throws BragaException;

    static native int init(byte[] configPath, byte[] keyPath) throws BragaException;

    /** Extracts both libraries to one temp directory and returns the shim's path. */
    private static Path extractNative() throws IOException {
        byte[] shim = readResource("libbraga_jni." + EXTENSION);
        byte[] cdylib = readResource("libbraga_ffi." + EXTENSION);
        Path directory = Files.createTempDirectory("braga-native-" + digest(shim, cdylib));
        directory.toFile().deleteOnExit();
        write(directory.resolve("libbraga_ffi." + EXTENSION), cdylib);
        return write(directory.resolve("libbraga_jni." + EXTENSION), shim);
    }

    private static byte[] readResource(String name) throws IOException {
        String path = "/native/" + DIRECTORY + "/" + name;
        try (InputStream stream = Native.class.getResourceAsStream(path)) {
            if (stream == null) {
                throw new IOException("missing native resource " + path
                        + "; build it with: cargo build --release -p braga-ffi && gradle test");
            }
            return stream.readAllBytes();
        }
    }

    private static Path write(Path file, byte[] contents) throws IOException {
        Files.write(file, contents);
        try {
            Files.setPosixFilePermissions(file, PosixFilePermissions.fromString("rwx------"));
        } catch (UnsupportedOperationException ignored) {
            // Windows keeps default permissions; v1 targets macOS and Linux anyway.
        }
        file.toFile().deleteOnExit();
        return file;
    }

    private static String digest(byte[]... parts) {
        try {
            MessageDigest sha = MessageDigest.getInstance("SHA-256");
            for (byte[] part : parts) {
                sha.update(part);
            }
            return HexFormat.of().formatHex(sha.digest(), 0, 8);
        } catch (NoSuchAlgorithmException error) {
            throw new IllegalStateException("SHA-256 is unavailable", error);
        }
    }

    private static String resourceDirectory() {
        String os = System.getProperty("os.name", "").toLowerCase(Locale.ROOT);
        String arch = System.getProperty("os.arch", "").toLowerCase(Locale.ROOT);
        String osDir = os.contains("mac") ? "macos" : (os.contains("linux") ? "linux" : null);
        String archDir = switch (arch) {
            case "aarch64", "arm64" -> "aarch64";
            case "x86_64", "amd64" -> "x86_64";
            default -> null;
        };
        if (osDir == null || archDir == null) {
            throw new UnsupportedOperationException(
                    "braga-jvm supports macOS and Linux on aarch64/x86_64; got " + os + "/" + arch);
        }
        return osDir + "-" + archDir;
    }
}
