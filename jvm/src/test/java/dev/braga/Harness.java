// SPDX-License-Identifier: Apache-2.0

package dev.braga;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.ObjectMapper;
import com.fasterxml.jackson.databind.node.ArrayNode;
import com.fasterxml.jackson.databind.node.ObjectNode;
import com.sun.net.httpserver.HttpExchange;
import com.sun.net.httpserver.HttpServer;
import java.io.IOException;
import java.net.InetSocketAddress;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Arrays;

/**
 * A hermetic fixture: a temporary tree with a sample document, a key file, and a loopback
 * OpenAI-compatible embedding stub, mirroring the Rust golden harness.
 */
final class Harness implements AutoCloseable {

    /** The sample document the semantic round-trip indexes. */
    static final String SAMPLE = "# Notes\n\nThe quick brown fox jumps over the lazy dog.\n";

    /** The query text that must find {@link #SAMPLE}. */
    static final String QUERY = "quick brown fox";

    private static final ObjectMapper JSON = new ObjectMapper();

    final Path root;
    final Path config;
    final Path key;
    private final HttpServer server;

    /** Creates {@code docs/notes.md}, a key file, and a stub-backed configuration under root. */
    Harness(Path root) throws IOException {
        this.root = root;
        Path docs = Files.createDirectories(root.resolve("docs"));
        Files.writeString(docs.resolve("notes.md"), SAMPLE, StandardCharsets.UTF_8);

        this.key = root.resolve("key");
        byte[] keyBytes = new byte[32];
        Arrays.fill(keyBytes, (byte) 0x5A);
        Files.write(key, keyBytes);

        this.server = HttpServer.create(new InetSocketAddress("127.0.0.1", 0), 0);
        server.createContext("/v1/embeddings", this::embeddings);
        server.start();

        Path database = root.resolve("index.db");
        String url = "http://127.0.0.1:" + server.getAddress().getPort() + "/v1/embeddings";
        String body = "[scan]\ndir = " + toml(docs.toString())
                + "\n\n[embedding]\nprovider = \"openai-compatible\"\nurl = " + toml(url)
                + "\nmodel = \"stub-embed\"\n\n[storage]\npath = " + toml(database.toString()) + "\n";
        this.config = root.resolve("braga.toml");
        Files.writeString(config, body, StandardCharsets.UTF_8);
    }

    /** Writes another file in the fixture root and returns its path. */
    Path write(String name, byte[] contents) throws IOException {
        return Files.write(root.resolve(name), contents);
    }

    /** Answers every embeddings request with one {@code [1,0,0,0]} vector per input. */
    private void embeddings(HttpExchange exchange) throws IOException {
        JsonNode request = JSON.readTree(exchange.getRequestBody());
        ArrayNode data = JSON.createArrayNode();
        for (int index = 0; index < request.path("input").size(); index++) {
            ObjectNode entry = data.addObject();
            entry.put("index", index);
            entry.putArray("embedding").add(1.0).add(0.0).add(0.0).add(0.0);
        }
        ObjectNode response = JSON.createObjectNode();
        response.set("data", data);

        byte[] body = JSON.writeValueAsBytes(response);
        exchange.getResponseHeaders().set("Content-Type", "application/json");
        exchange.sendResponseHeaders(200, body.length);
        exchange.getResponseBody().write(body);
        exchange.close();
    }

    /** Renders a TOML basic string for a path or URL. */
    private static String toml(String value) {
        return "\"" + value.replace("\\", "\\\\").replace("\"", "\\\"") + "\"";
    }

    @Override
    public void close() {
        server.stop(0);
    }
}
