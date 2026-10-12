package tool

import (
	"encoding/json/jsontext"
	"encoding/json/v2"
	"errors"
	"fmt"

	"github.com/gfhfyjbr/kou-conveyor/harness/operation"
	"github.com/gfhfyjbr/kou-conveyor/harness/tool/toolarg"
)

func ErrorStatus(message string, limit int) CallStatus {
	if limit <= 0 {
		limit = operation.DefaultMaxOutputLength
	}
	message, truncated := operation.BoundOutput(message, limit)
	return CallStatus{Error: message, ErrorTruncated: truncated}
}

// ParseMaxOutputLength reads max_output_length, which models write as 100
// or "100" alike.
func ParseMaxOutputLength(encoded jsontext.Value) (int, error) {
	if len(encoded) == 0 {
		return operation.DefaultMaxOutputLength, nil
	}
	var number toolarg.Number
	if err := json.Unmarshal(encoded, &number); err != nil {
		return 0, fmt.Errorf("decode max_output_length: %w", err)
	}
	limit, whole := number.Whole()
	if !whole || limit <= 0 {
		return 0, errors.New("max_output_length must be a positive integer")
	}
	if limit > operation.MaxOutputLength {
		return 0, fmt.Errorf("max_output_length must not exceed %d", operation.MaxOutputLength)
	}
	return limit, nil
}
