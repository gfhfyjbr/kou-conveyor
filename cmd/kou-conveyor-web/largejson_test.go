package main

import (
	"bytes"
	"compress/gzip"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json/v2"
	"fmt"
	"io"
	"math/rand/v2"
	"net/http"
	"net/http/httptest"
	"testing"
)

func TestWriteLargeJSON(t *testing.T) {
	// Lines that compress little, so that the compressed JSON too takes
	// more than a chunk.
	lines := make([]string, 6000)
	for i := range lines {
		sum := sha256.Sum256(fmt.Append(nil, i))
		lines[i] = hex.EncodeToString(sum[:])
	}
	for name, v := range map[string]any{
		"small": map[string]any{"id": "s", "entries": []string{"one", "two"}},
		"large": map[string]any{"id": "l", "entries": lines},
	} {
		want, err := json.Marshal(v, json.Deterministic(true))
		if err != nil {
			t.Fatal(err)
		}
		sum := sha256.Sum256(want)
		tag := `"` + hex.EncodeToString(sum[:12]) + `"`
		for _, encoding := range []string{"gzip, br", "identity"} {
			req := httptest.NewRequest("GET", "/api/sessions/x", nil)
			req.Header.Set("Accept-Encoding", encoding)
			rec := httptest.NewRecorder()
			writeLargeJSON(rec, req, v)
			got := rec.Body.Bytes()
			compressed := encoding != "identity" && len(want) >= 4<<10
			if (rec.Header().Get("Content-Encoding") == "gzip") != compressed {
				t.Fatalf("%s, %s: Content-Encoding %q", name, encoding, rec.Header().Get("Content-Encoding"))
			}
			if compressed {
				zr, err := gzip.NewReader(bytes.NewReader(got))
				if err != nil {
					t.Fatal(err)
				}
				if got, err = io.ReadAll(zr); err != nil {
					t.Fatal(err)
				}
			}
			if rec.Code != http.StatusOK || !bytes.Equal(got, want) || rec.Header().Get("ETag") != tag {
				t.Fatalf("%s, %s: %d, etag %s, want %s; the JSON alike: %v (%d bytes of %d)",
					name, encoding, rec.Code, rec.Header().Get("ETag"), tag, bytes.Equal(got, want), len(got), len(want))
			}
			req.Header.Set("If-None-Match", tag)
			rec = httptest.NewRecorder()
			writeLargeJSON(rec, req, v)
			if rec.Code != http.StatusNotModified || rec.Body.Len() != 0 {
				t.Fatalf("%s, %s, had already: %d, %d bytes", name, encoding, rec.Code, rec.Body.Len())
			}
		}
	}
}

func TestChunksReadWhatWasWritten(t *testing.T) {
	random := rand.NewChaCha8([32]byte{1})
	var c chunks
	var want []byte
	for _, n := range []int{1, chunkSize - 1, 1, 0, chunkSize, 2*chunkSize + 3} {
		piece := make([]byte, n)
		random.Read(piece)
		if written, err := c.Write(piece); written != n || err != nil {
			t.Fatalf("Write of %d: %d, %v", n, written, err)
		}
		want = append(want, piece...)
	}
	got, err := io.ReadAll(&c)
	if err != nil || !bytes.Equal(got, want) {
		t.Fatalf("read %d bytes of %d, alike %v, %v", len(got), len(want), bytes.Equal(got, want), err)
	}
	if len(c.pieces) != 0 {
		t.Fatalf("%d pieces kept once read", len(c.pieces))
	}
	if n, err := c.Read(make([]byte, 8)); n != 0 || err != io.EOF {
		t.Fatalf("read past the end: %d, %v", n, err)
	}
}
