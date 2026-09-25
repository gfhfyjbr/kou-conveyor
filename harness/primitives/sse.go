package primitives

import "bytes"

// SSEData extracts data fields from one already-framed SSE event.
// Nil means no data fields; an empty data field returns a non-nil empty slice.
func SSEData(frame []byte) []byte {
	frame = bytes.TrimPrefix(frame, []byte("\ufeff"))
	var data []byte
	for _, line := range bytes.FieldsFunc(frame, func(r rune) bool { return r == '\r' || r == '\n' }) {
		field, value, _ := bytes.Cut(line, []byte(":"))
		if !bytes.Equal(field, []byte("data")) {
			continue
		}
		value = bytes.TrimPrefix(value, []byte(" "))
		data = append(data, value...)
		data = append(data, '\n')
	}
	if data == nil {
		return nil
	}
	return data[:len(data)-1]
}
