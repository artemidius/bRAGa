// SPDX-License-Identifier: Apache-2.0

package braga

import (
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
)

// sample is the document the semantic round-trip indexes.
const sample = "# Notes\n\nThe quick brown fox jumps over the lazy dog.\n"

// query is the text that must find sample.
const query = "quick brown fox"

// fixture is a hermetic tree rooted in one t.TempDir: docs, a key, and a stub-backed config.
type fixture struct {
	root   string
	config string
	key    string
}

// newFixture creates docs/notes.md, a 32-byte key file, and a config that embeds through a
// loopback httptest server, mirroring crates/braga-ffi/tests/golden/harness.rs.
func newFixture(t *testing.T) *fixture {
	t.Helper()
	root := t.TempDir()

	docs := filepath.Join(root, "docs")
	if err := os.Mkdir(docs, 0o755); err != nil {
		t.Fatalf("create scan directory: %v", err)
	}
	if err := os.WriteFile(filepath.Join(docs, "notes.md"), []byte(sample), 0o644); err != nil {
		t.Fatalf("write sample document: %v", err)
	}

	key := filepath.Join(root, "key")
	keyBytes := make([]byte, 32)
	for index := range keyBytes {
		keyBytes[index] = 0x5A
	}
	if err := os.WriteFile(key, keyBytes, 0o600); err != nil {
		t.Fatalf("write key file: %v", err)
	}

	stub := httptest.NewServer(http.HandlerFunc(embedHandler))
	t.Cleanup(stub.Close)

	config := filepath.Join(root, "braga.toml")
	url := stub.URL + "/v1/embeddings"
	db := filepath.Join(root, "index.db")
	body := fmt.Sprintf(
		"[scan]\ndir = %s\n\n[embedding]\nprovider = \"openai-compatible\"\nurl = %s\n"+
			"model = \"stub-embed\"\n\n[storage]\npath = %s\n",
		strconv.Quote(docs), strconv.Quote(url), strconv.Quote(db),
	)
	if err := os.WriteFile(config, []byte(body), 0o644); err != nil {
		t.Fatalf("write config file: %v", err)
	}

	return &fixture{root: root, config: config, key: key}
}

// write adds another file in the fixture root and returns its path.
func (f *fixture) write(t *testing.T, name string, contents []byte) string {
	t.Helper()
	path := filepath.Join(f.root, name)
	if err := os.WriteFile(path, contents, 0o644); err != nil {
		t.Fatalf("write fixture file %s: %v", name, err)
	}
	return path
}

// embedHandler answers every OpenAI-compatible embeddings request with one [1,0,0,0] vector per
// input, the Rust golden vector.
func embedHandler(w http.ResponseWriter, r *http.Request) {
	var request struct {
		Input []json.RawMessage `json:"input"`
	}
	body, err := io.ReadAll(r.Body)
	if err != nil {
		http.Error(w, "read request", http.StatusBadRequest)
		return
	}
	_ = json.Unmarshal(body, &request)

	data := make([]map[string]any, len(request.Input))
	for index := range data {
		data[index] = map[string]any{"index": index, "embedding": []float64{1, 0, 0, 0}}
	}
	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(map[string]any{"data": data})
}

// requireBragaError asserts that err is a BragaError with the wanted result code.
func requireBragaError(t *testing.T, err error, want Result) *BragaError {
	t.Helper()
	if err == nil {
		t.Fatalf("expected a BragaError with code %s, got nil", want)
	}
	var target *BragaError
	if !errors.As(err, &target) {
		t.Fatalf("expected *BragaError, got %T: %v", err, err)
	}
	if target.Code != want {
		t.Fatalf("expected code %s, got %s (%v)", want, target.Code, err)
	}
	return target
}

// requireBragaMessage asserts the error's code and that its sanitized message mentions substring.
func requireBragaMessage(t *testing.T, err error, want Result, substring string) *BragaError {
	t.Helper()
	target := requireBragaError(t, err, want)
	if !strings.Contains(target.Message, substring) {
		t.Fatalf("message = %q, want it to contain %q", target.Message, substring)
	}
	return target
}
