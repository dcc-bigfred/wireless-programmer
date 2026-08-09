package client

import (
	"bytes"
	"encoding/binary"
	"testing"
)

func TestWriteFullShortWrites(t *testing.T) {
	var buf bytes.Buffer
	w := &shortWriter{w: &buf, max: 3}
	if err := writeFrame(w, map[string]string{"type": "hello"}); err != nil {
		t.Fatal(err)
	}
	var resp Response
	if err := readFrame(bytes.NewReader(buf.Bytes()), &resp); err != nil {
		t.Fatal(err)
	}
	if resp.Type != "hello" {
		t.Fatalf("got %+v", resp)
	}
}

type shortWriter struct {
	w   *bytes.Buffer
	max int
}

func (s *shortWriter) Write(p []byte) (int, error) {
	if len(p) > s.max {
		p = p[:s.max]
	}
	return s.w.Write(p)
}

func TestReadFrameExactLength(t *testing.T) {
	var buf bytes.Buffer
	if err := writeFrame(&buf, map[string]any{"type": "hello"}); err != nil {
		t.Fatal(err)
	}
	if err := writeFrame(&buf, map[string]any{"type": "scan", "result": []any{}}); err != nil {
		t.Fatal(err)
	}
	r := bytes.NewReader(buf.Bytes())
	var a, b Response
	if err := readFrame(r, &a); err != nil {
		t.Fatal(err)
	}
	if a.Type != "hello" {
		t.Fatalf("first: %+v", a)
	}
	if err := readFrame(r, &b); err != nil {
		t.Fatal(err)
	}
	if b.Type != "scan" {
		t.Fatalf("second: %+v", b)
	}
}

func TestReadFrameRejectsOversized(t *testing.T) {
	var buf bytes.Buffer
	// Declare a 2 MiB payload length, far over the 1 MiB cap.
	var hdr [4]byte
	binary.LittleEndian.PutUint32(hdr[:], uint32(2*1024*1024))
	buf.Write(hdr[:])
	var resp Response
	if err := readFrame(bytes.NewReader(buf.Bytes()), &resp); err == nil {
		t.Fatal("expected oversized error")
	}
}
