package primitives

import (
	"bytes"
	"fmt"
	"net/http"
	"net/http/httptest"
	"reflect"
	"strings"
	"testing"
)

func TestSSEData(t *testing.T) {
	for _, test := range []struct {
		name, frame, want string
		absent            bool
	}{
		{name: "empty", absent: true},
		{name: "separators only", frame: "\n\n", absent: true},
		{name: "comment", frame: ": data: ignored", absent: true},
		{name: "other fields", frame: "event: message\nid: 1\nretry: 20\nunknown: value", absent: true},
		{name: "bare unknown field", frame: "unknown", absent: true},
		{name: "single", frame: "data: hello", want: "hello"},
		{name: "no space", frame: "data:hello", want: "hello"},
		{name: "one space only", frame: "data:  hello", want: " hello"},
		{name: "leading tab", frame: "data:\thello", want: "\thello"},
		{name: "trailing whitespace", frame: "data: hello \t", want: "hello \t"},
		{name: "colons in value", frame: "data: https://example.test:123/:foo", want: "https://example.test:123/:foo"},
		{name: "case sensitive", frame: "Data: ignored\nDATA: ignored\ndata: kept", want: "kept"},
		{name: "field whitespace", frame: " data: ignored\ndata : ignored\ndata\t: ignored", absent: true},
		{name: "empty data", frame: "data:"},
		{name: "empty data with space", frame: "data: "},
		{name: "bare data", frame: "data"},
		{name: "whitespace data", frame: "data:  \t", want: " \t"},
		{name: "multiple fields", frame: "data: first\ndata: second", want: "first\nsecond"},
		{name: "leading empty", frame: "data:\ndata: next", want: "\nnext"},
		{name: "bare leading empty", frame: "data\ndata: next", want: "\nnext"},
		{name: "middle empty", frame: "data: first\ndata\ndata: last", want: "first\n\nlast"},
		{name: "trailing empty", frame: "data: first\ndata:", want: "first\n"},
		{name: "all empty", frame: "data:\ndata\ndata: ", want: "\n\n"},
		{name: "interleaved fields", frame: "data: first\n: comment\nevent: message\nid: 1\ndata: second", want: "first\nsecond"},
		{name: "no implicit continuation", frame: "data: first\nnot a data line\ndata: second", want: "first\nsecond"},
		{name: "delimiter retained", frame: "data: first\ndata: second\n\n", want: "first\nsecond"},
		{name: "multiline JSON", frame: "event: response.completed\ndata: {\"type\":\"response.completed\",\ndata: \"response\":{}}", want: "{\"type\":\"response.completed\",\n\"response\":{}}"},
		{name: "escaped newline", frame: `data: {"text":"first\nsecond"}`, want: `{"text":"first\nsecond"}`},
		{name: "Unicode", frame: "data: 会話\u2028café\u2029", want: "会話\u2028café\u2029"},
		{name: "NUL in data", frame: "data: a\x00b", want: "a\x00b"},
		{name: "BOM in data", frame: "data: \ufefftext", want: "\ufefftext"},
		{name: "leading BOM", frame: "\ufeffdata: hello", want: "hello"},
		{name: "leading BOM and BOM in data", frame: "\ufeffdata: \ufefftext", want: "\ufefftext"},
		{name: "opaque bytes", frame: "data: \xff\xfe", want: "\xff\xfe"},
		{name: "JSON-RPC message", frame: `event: message
id: session/stream/1
data: {"jsonrpc":"2.0","id":1,"result":{}}`, want: `{"jsonrpc":"2.0","id":1,"result":{}}`},
		{name: "application sentinel", frame: "data: [DONE]", want: "[DONE]"},
	} {
		for _, ending := range []string{"\n", "\r\n", "\r"} {
			t.Run(test.name+fmt.Sprintf("/%q", ending), func(t *testing.T) {
				frame := []byte(strings.ReplaceAll(test.frame, "\n", ending))
				before := bytes.Clone(frame)
				got := SSEData(frame)
				if string(got) != test.want || (got == nil) != test.absent {
					t.Fatalf("SSEData(%q) = %q (nil=%v), want %q (nil=%v)", frame, got, got == nil, test.want, test.absent)
				}
				if !bytes.Equal(frame, before) {
					t.Fatal("input mutated")
				}
			})
		}
	}
}

func TestSSEDataMixedLineEndings(t *testing.T) {
	got := SSEData([]byte("event: message\r\ndata: one\ndata:\rdata: two\r\n"))
	if string(got) != "one\n\ntwo" {
		t.Fatalf("data = %q", got)
	}
}

func TestSSEDataOwnsOutput(t *testing.T) {
	frame := []byte("data: original")
	data := SSEData(frame)
	data[0] = 'X'
	if string(frame) != "data: original" {
		t.Fatal("output aliases input")
	}
	frame[6] = 'Y'
	if string(data) != "Xriginal" {
		t.Fatal("input aliases output")
	}
	if got := SSEData([]byte(": comment")); got != nil {
		t.Fatal("data leaked between frames")
	}
}

func TestSSEDataLargeField(t *testing.T) {
	want := bytes.Repeat([]byte("x"), 1<<20)
	frame := append([]byte("data: "), want...)
	if got := SSEData(frame); !bytes.Equal(got, want) {
		t.Fatalf("large field length = %d, want %d", len(got), len(want))
	}
}

func TestSSEDataWithFramerChunkBoundaries(t *testing.T) {
	for _, ending := range []string{"\n", "\r\n", "\r"} {
		wire := []byte(strings.ReplaceAll("\ufeffdata: first\n\n: heartbeat\n\n\ufeffdata\n\ndata:\ndata: hello\n: comment\ndata:\n\ndata: [DONE]\n\ndata: unfinished", "\n", ending))
		want := [][]byte{[]byte("first"), nil, {}, []byte("\nhello\n"), []byte("[DONE]")}
		for split := 0; split <= len(wire); split++ {
			framer := newRemoteSSEFramer(int64(len(wire)), SSEFrameDelimiterStrip)
			var got [][]byte
			for _, chunk := range [][]byte{wire[:split], wire[split:]} {
				frames, err := framer.append(chunk)
				if err != nil {
					t.Fatal(err)
				}
				for _, frame := range frames {
					got = append(got, SSEData(frame.data))
				}
			}
			for _, frame := range framer.finish() {
				got = append(got, SSEData(frame.data))
			}
			if !reflect.DeepEqual(got, want) {
				t.Fatalf("ending=%q split=%d: got %#v, want %#v", ending, split, got, want)
			}
		}
	}
}

func TestSSEDataFromRemotePrimitive(t *testing.T) {
	const wire = ": comment\r\rdata:\rdata: first\r\ndata: last\n\ndata\n\ndata: [DONE]\n\ndata: unfinished"
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.Header().Set("Content-Type", "text/event-stream")
		for i := range len(wire) {
			if _, err := w.Write([]byte{wire[i]}); err != nil {
				t.Error(err)
				return
			}
			w.(http.Flusher).Flush()
		}
	}))
	defer server.Close()
	request := DefaultRemoteRequest("sse", "fields", server.URL)
	request.SSE = &RemoteSSEOptions{MaxFrameSize: 1024, FrameDelimiter: SSEFrameDelimiterStrip}
	events := collectInternalEvents(sendTestRemoteRequest(t, t.Context(), request))
	var got [][]byte
	for _, event := range remoteEventsOfType(events, PrimitiveEventRemoteOutput) {
		got = append(got, SSEData(event.Result.(RemoteOutputResult).Data))
	}
	want := [][]byte{nil, []byte("\nfirst\nlast"), {}, []byte("[DONE]")}
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("data = %#v, want %#v", got, want)
	}
	if events[len(events)-1].Type != PrimitiveEventRemoteCompleted {
		t.Fatalf("terminal event = %#v", events[len(events)-1])
	}
}

func FuzzSSEDataRoundTrip(f *testing.F) {
	for _, seed := range []string{"", "hello", "\nleading", "trailing\n", "\n\n", ": colons: preserved", " \t whitespace \t", "会話\u2028", "\xff\x00", "[DONE]"} {
		f.Add([]byte(seed), uint8(0), uint8(1))
	}
	f.Fuzz(func(t *testing.T, data []byte, endingIndex, chunkSize uint8) {
		if len(data) > 1<<20 {
			t.Skip()
		}
		want := bytes.ReplaceAll(bytes.ReplaceAll(data, []byte("\r\n"), []byte("\n")), []byte("\r"), []byte("\n"))
		ending := []string{"\n", "\r\n", "\r"}[int(endingIndex)%3]
		var frame bytes.Buffer
		frame.WriteString("event: message" + ending)
		for line := range bytes.SplitSeq(want, []byte("\n")) {
			frame.WriteString("data: ")
			frame.Write(line)
			frame.WriteString(ending)
		}
		frame.WriteString(ending)
		wire := frame.Bytes()
		before := bytes.Clone(wire)
		if got := SSEData(wire); got == nil || !bytes.Equal(got, want) {
			t.Fatalf("round trip = %q, want %q", got, want)
		}
		if !bytes.Equal(wire, before) {
			t.Fatal("input mutated")
		}
		framer := newRemoteSSEFramer(int64(len(wire)), SSEFrameDelimiterStrip)
		var frames []remoteOutputFrame
		for i := 0; i < len(wire); i += int(chunkSize) + 1 {
			out, err := framer.append(wire[i:min(i+int(chunkSize)+1, len(wire))])
			if err != nil {
				t.Fatal(err)
			}
			frames = append(frames, out...)
		}
		frames = append(frames, framer.finish()...)
		if len(frames) != 1 {
			t.Fatalf("frames = %d", len(frames))
		}
		if got := SSEData(frames[0].data); got == nil || !bytes.Equal(got, want) {
			t.Fatalf("framed round trip = %q, want %q", got, want)
		}
	})
}
