// SPDX-License-Identifier: Apache-2.0

package braga

import (
	"bytes"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"testing"
)

func TestAbiVersion(t *testing.T) {
	if got := AbiVersion(); got != 1 {
		t.Fatalf("AbiVersion() = %d, want 1", got)
	}
	if got := _braga_abi_version(); got != 1 {
		t.Fatalf("_braga_abi_version() = %d, want 1", got)
	}
}

func TestEnumsMatchHeader(t *testing.T) {
	results := map[Result]int{
		ResultOK: 0, ResultInvalidArg: 1, ResultConfig: 2, ResultKey: 3, ResultStore: 4,
		ResultEmbed: 5, ResultNotFound: 6, ResultIO: 7, ResultPanic: 8, ResultOOM: 9,
	}
	for got, want := range results {
		if int(got) != want {
			t.Errorf("Result %s = %d, want %d", got, int(got), want)
		}
	}

	statuses := map[Status]int{
		StatusUpToDate: 0, StatusChecking: 1, StatusUpdating: 2, StatusError: 3,
	}
	for got, want := range statuses {
		if int(got) != want {
			t.Errorf("Status %s = %d, want %d", got, int(got), want)
		}
	}
}

func TestRoundTrip(t *testing.T) {
	fixture := newFixture(t)
	lib, err := Open(fixture.config, fixture.key)
	if err != nil {
		t.Fatalf("Open: %v", err)
	}

	if got := lib.Status(); got != StatusUpToDate {
		t.Fatalf("Status after open = %s, want UP_TO_DATE", got)
	}
	if _, err := lib.Query(query, 1); err != nil {
		requireBragaError(t, err, ResultNotFound)
	}

	if err := lib.Update(); err != nil {
		t.Fatalf("Update: %v", err)
	}
	if got := lib.Status(); got != StatusUpToDate {
		t.Fatalf("Status after update = %s, want UP_TO_DATE", got)
	}

	hits, err := lib.Query(query, 1)
	if err != nil {
		t.Fatalf("Query: %v", err)
	}
	if len(hits) != 1 {
		t.Fatalf("Query returned %d hits, want 1", len(hits))
	}
	hit := hits[0]
	if !regexp.MustCompile(`^[0-9a-f]{64}$`).MatchString(hit.ChunkID) {
		t.Errorf("chunk_id = %q, want lowercase hex SHA-256", hit.ChunkID)
	}
	if !strings.HasSuffix(hit.Path, "notes.md") {
		t.Errorf("path = %q, want it to end with notes.md", hit.Path)
	}
	if !strings.Contains(hit.Content, "quick brown fox") {
		t.Errorf("content = %q, want the sample sentence", hit.Content)
	}
	if hit.LineStart < 1 || hit.LineEnd < hit.LineStart {
		t.Errorf("line range = %d..%d, want a sane range", hit.LineStart, hit.LineEnd)
	}
	if hit.Score <= 0 {
		t.Errorf("score = %v, want > 0", hit.Score)
	}

	report, err := lib.Doctor()
	if err != nil {
		t.Fatalf("Doctor: %v", err)
	}
	if !report.Ok {
		t.Errorf("doctor report ok = false: %+v", report.Checks)
	}
	if len(report.Checks) != 6 {
		t.Errorf("doctor reported %d checks, want 6", len(report.Checks))
	}
	for _, check := range report.Checks {
		if !check.Passed {
			t.Errorf("doctor check %s failed: %s", check.Name, check.Detail)
		}
	}

	if err := lib.Close(); err != nil {
		t.Fatalf("Close: %v", err)
	}
	if err := lib.Close(); err != nil {
		t.Fatalf("second Close: %v", err)
	}
	_, err = lib.Query(query, 1)
	requireBragaError(t, err, ResultInvalidArg)
	if got := lib.Status(); got != StatusError {
		t.Errorf("Status after close = %s, want ERROR", got)
	}
}

func TestWrongKey(t *testing.T) {
	fixture := newFixture(t)
	lib, err := Open(fixture.config, fixture.key)
	if err != nil {
		t.Fatalf("Open: %v", err)
	}
	if err := lib.Close(); err != nil {
		t.Fatalf("Close: %v", err)
	}

	wrong := fixture.write(t, "wrong-key", bytes.Repeat([]byte{0x11}, 32))
	_, err = Open(fixture.config, wrong)
	requireBragaMessage(t, err, ResultStore, "wrong database key")
}

func TestMalformedKey(t *testing.T) {
	fixture := newFixture(t)
	bad := fixture.write(t, "bad-key", []byte("not-a-key"))

	_, err := Open(fixture.config, bad)
	bragaErr := requireBragaMessage(t, err, ResultKey, "expected exactly 32 bytes")
	if strings.Contains(bragaErr.Error(), "not-a-key") {
		t.Errorf("key content leaked into the message: %q", bragaErr.Message)
	}
}

func TestMissingConfig(t *testing.T) {
	fixture := newFixture(t)
	_, err := Open(filepath.Join(fixture.root, "missing.toml"), fixture.key)
	requireBragaMessage(t, err, ResultConfig, "configuration")
}

func TestBrokenToml(t *testing.T) {
	fixture := newFixture(t)
	broken := fixture.write(t, "broken.toml", []byte("[scan]\ndir = \"SUPER-SECRET\" nope\n"))

	_, err := Open(broken, fixture.key)
	bragaErr := requireBragaError(t, err, ResultConfig)
	if bragaErr.Message != "invalid configuration" {
		t.Errorf("message = %q, want %q", bragaErr.Message, "invalid configuration")
	}
	if strings.Contains(bragaErr.Error(), "SUPER-SECRET") {
		t.Errorf("file content leaked into the message: %q", bragaErr.Message)
	}
}

func TestInitRefusesToClobber(t *testing.T) {
	fixture := newFixture(t)
	config := filepath.Join(fixture.root, "init.toml")
	key := filepath.Join(fixture.root, "nested", "init-key")

	if err := Init(config, key); err != nil {
		t.Fatalf("Init: %v", err)
	}
	if _, err := os.Stat(config); err != nil {
		t.Fatalf("init did not write the config: %v", err)
	}
	keyBytes, err := os.ReadFile(key)
	if err != nil {
		t.Fatalf("read generated key: %v", err)
	}
	if len(keyBytes) != 32 {
		t.Errorf("generated key is %d bytes, want 32", len(keyBytes))
	}

	err = Init(config, key)
	requireBragaMessage(t, err, ResultIO, "already exists")
}

func TestNoCrash(t *testing.T) {
	if code, _ := _braga_close(nil); code != ResultInvalidArg {
		t.Errorf("close(NULL) = %s, want INVALID_ARG", code)
	}
	if got := _braga_status(nil); got != StatusError {
		t.Errorf("status(NULL) = %s, want ERROR", got)
	}
	if code, _ := _braga_update(nil); code != ResultInvalidArg {
		t.Errorf("update(NULL) = %s, want INVALID_ARG", code)
	}
	if code, _, _ := _braga_doctor(nil); code != ResultInvalidArg {
		t.Errorf("doctor(NULL) = %s, want INVALID_ARG", code)
	}
	if code, _, _ := _braga_query(nil, []byte(query), 1); code != ResultInvalidArg {
		t.Errorf("query(NULL) = %s, want INVALID_ARG", code)
	}

	fixture := newFixture(t)
	lib, err := Open(fixture.config, fixture.key)
	if err != nil {
		t.Fatalf("Open: %v", err)
	}
	raw := lib.handle

	for name, text := range map[string][]byte{
		"NULL":          nil,
		"empty":         {},
		"invalid UTF-8": {0xFF, 0xFE},
	} {
		if code, _, _ := _braga_query(raw, text, 1); code != ResultInvalidArg {
			t.Errorf("query(%s text) = %s, want INVALID_ARG", name, code)
		}
	}

	if code, _ := _braga_update(raw); code != ResultOK {
		t.Fatalf("update = %s, want OK", code)
	}
	code, data, _ := _braga_query(raw, []byte(query), 1)
	if code != ResultOK || data == "" {
		t.Fatalf("query = %s, json length %d, want OK with JSON", code, len(data))
	}

	code, data, _ = _braga_query(raw, []byte(query), 0xFFFFFFFF)
	if code == ResultPanic {
		t.Errorf("oversized top_k panicked")
	}
	if (data != "") != (code == ResultOK) {
		t.Errorf("oversized top_k returned code %s with %d bytes of JSON", code, len(data))
	}

	if err := lib.Close(); err != nil {
		t.Fatalf("Close: %v", err)
	}
}
