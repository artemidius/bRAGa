// SPDX-License-Identifier: Apache-2.0

package dev.braga;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Arrays;
import java.util.List;
import java.util.regex.Pattern;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.function.Executable;
import org.junit.jupiter.api.io.TempDir;

/** Conformance tests against the real cdylib, mirroring go/braga_test.go. */
final class BragaTest {

    private static final Pattern CHUNK_ID = Pattern.compile("[0-9a-f]{64}");
    private static final byte[] QUERY = Harness.QUERY.getBytes(StandardCharsets.UTF_8);

    @TempDir
    Path root;

    @Test
    void abiVersionIsOne() {
        assertEquals(1, Braga.abiVersion());
    }

    @Test
    void enumsMatchHeader() {
        int[] results = {0, 1, 2, 3, 4, 5, 6, 7, 8, 9};
        BragaResult[] values = BragaResult.values();
        assertEquals(results.length, values.length);
        for (int index = 0; index < results.length; index++) {
            assertEquals(results[index], values[index].code(), values[index].name());
        }

        int[] statuses = {0, 1, 2, 3};
        BragaStatus[] states = BragaStatus.values();
        assertEquals(statuses.length, states.length);
        for (int index = 0; index < statuses.length; index++) {
            assertEquals(statuses[index], states[index].code(), states[index].name());
        }

        assertEquals(BragaStatus.ERROR, BragaStatus.fromCode(99));
        assertEquals(BragaResult.PANIC, BragaResult.fromCode(99));
    }

    @Test
    void roundTrip() throws Exception {
        try (Harness harness = new Harness(root)) {
            Braga braga = Braga.open(harness.config.toString(), harness.key.toString());
            assertEquals(BragaStatus.UP_TO_DATE, braga.status());
            assertBragaError(() -> braga.query(Harness.QUERY, 1), BragaResult.NOT_FOUND);

            braga.update();
            assertEquals(BragaStatus.UP_TO_DATE, braga.status());

            List<Hit> hits = braga.query(Harness.QUERY, 1);
            assertEquals(1, hits.size());
            Hit hit = hits.get(0);
            assertTrue(CHUNK_ID.matcher(hit.chunkId()).matches(), hit.chunkId());
            assertTrue(hit.path().endsWith("notes.md"), hit.path());
            assertTrue(hit.content().contains("quick brown fox"), hit.content());
            assertTrue(hit.lineStart() >= 1 && hit.lineEnd() >= hit.lineStart(),
                    hit.lineStart() + ".." + hit.lineEnd());
            assertTrue(hit.score() > 0, Double.toString(hit.score()));

            DoctorReport report = braga.doctor();
            assertTrue(report.ok(), report.checks().toString());
            assertEquals(6, report.checks().size());
            for (DoctorCheck check : report.checks()) {
                assertTrue(check.passed(), check.name() + ": " + check.detail());
            }

            braga.close();
            braga.close();
            assertBragaError(() -> braga.query(Harness.QUERY, 1), BragaResult.INVALID_ARG);
            assertEquals(BragaStatus.ERROR, braga.status());
        }
    }

    @Test
    void wrongKey() throws Exception {
        try (Harness harness = new Harness(root)) {
            Braga.open(harness.config.toString(), harness.key.toString()).close();

            byte[] wrong = new byte[32];
            Arrays.fill(wrong, (byte) 0x11);
            Path path = harness.write("wrong-key", wrong);
            BragaException error = assertBragaError(
                    () -> Braga.open(harness.config.toString(), path.toString()), BragaResult.STORE);
            assertTrue(error.getMessage().contains("wrong database key"), error.getMessage());
        }
    }

    @Test
    void malformedKey() throws Exception {
        try (Harness harness = new Harness(root)) {
            Path path = harness.write("bad-key", "not-a-key".getBytes(StandardCharsets.UTF_8));
            BragaException error = assertBragaError(
                    () -> Braga.open(harness.config.toString(), path.toString()), BragaResult.KEY);
            assertTrue(error.getMessage().contains("expected exactly 32 bytes"), error.getMessage());
            assertFalse(error.getMessage().contains("not-a-key"), error.getMessage());
        }
    }

    @Test
    void missingConfig() throws Exception {
        try (Harness harness = new Harness(root)) {
            Path missing = harness.root.resolve("missing.toml");
            BragaException error = assertBragaError(
                    () -> Braga.open(missing.toString(), harness.key.toString()),
                    BragaResult.CONFIG);
            assertTrue(error.getMessage().contains("configuration"), error.getMessage());
        }
    }

    @Test
    void brokenToml() throws Exception {
        try (Harness harness = new Harness(root)) {
            Path broken = harness.write("broken.toml",
                    "[scan]\ndir = \"SUPER-SECRET\" nope\n".getBytes(StandardCharsets.UTF_8));
            BragaException error = assertBragaError(
                    () -> Braga.open(broken.toString(), harness.key.toString()), BragaResult.CONFIG);
            assertEquals("CONFIG: invalid configuration", error.getMessage());
            assertFalse(error.getMessage().contains("SUPER-SECRET"), error.getMessage());
        }
    }

    @Test
    void initRefusesToClobber() throws Exception {
        Path config = root.resolve("init.toml");
        Path key = root.resolve("nested").resolve("init-key");

        Braga.init(config.toString(), key.toString());
        assertTrue(Files.isRegularFile(config), "init did not write the config");
        assertEquals(32, Files.size(key));

        BragaException error = assertBragaError(
                () -> Braga.init(config.toString(), key.toString()), BragaResult.IO);
        assertTrue(error.getMessage().contains("already exists"), error.getMessage());
    }

    @Test
    void noCrash() throws Exception {
        assertBragaError(() -> Native.close(0), BragaResult.INVALID_ARG);
        assertEquals(BragaStatus.ERROR, BragaStatus.fromCode(Native.status(0)));
        assertBragaError(() -> Native.update(0), BragaResult.INVALID_ARG);
        assertBragaError(() -> Native.doctor(0), BragaResult.INVALID_ARG);
        assertBragaError(() -> Native.query(0, QUERY, 1), BragaResult.INVALID_ARG);

        try (Harness harness = new Harness(root)) {
            Braga braga = Braga.open(harness.config.toString(), harness.key.toString());
            braga.update();
            long raw = braga.rawHandle();

            assertBragaError(() -> Native.query(raw, null, 1), BragaResult.INVALID_ARG);
            assertBragaError(() -> Native.query(raw, new byte[0], 1), BragaResult.INVALID_ARG);
            assertBragaError(() -> Native.query(raw, new byte[] {(byte) 0xFF, (byte) 0xFE}, 1),
                    BragaResult.INVALID_ARG);
            assertBragaError(() -> braga.query(Harness.QUERY, -1), BragaResult.INVALID_ARG);

            assertNotNull(Native.query(raw, QUERY, 1));
            try {
                assertNotNull(Native.query(raw, QUERY, -1));
            } catch (BragaException error) {
                assertNotEquals(BragaResult.PANIC, error.code(), "oversized topK panicked");
            }

            braga.close();
        }
    }

    private static BragaException assertBragaError(Executable call, BragaResult want) {
        BragaException error = assertThrows(BragaException.class, call);
        assertEquals(want, error.code(), error.getMessage());
        return error;
    }
}
