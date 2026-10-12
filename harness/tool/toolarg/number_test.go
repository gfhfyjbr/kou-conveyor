package toolarg_test

import (
	"encoding/json/v2"
	"strings"
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/harness/tool/toolarg"
)

func TestNumberReadsNumbersAsModelsWriteThem(t *testing.T) {
	for _, test := range []struct {
		json string
		want float64
	}{
		{`1860`, 1860}, {`"1860"`, 1860}, {`" 1860 "`, 1860}, {`1860.0`, 1860}, {`"1.5"`, 1.5}, {`-3`, -3}, {`"-3"`, -3},
	} {
		var n toolarg.Number
		if err := json.Unmarshal([]byte(test.json), &n); err != nil || float64(n) != test.want {
			t.Errorf("%s: %v, %v; want %v", test.json, float64(n), err, test.want)
		}
	}
	for _, bad := range []string{`"abc"`, `""`, `true`, `[1]`, `{"n":1}`, `"NaN"`, `"Inf"`} {
		var n toolarg.Number
		err := json.Unmarshal([]byte(bad), &n)
		if err == nil || !strings.Contains(err.Error(), "want a number") {
			t.Errorf("%s: %v", bad, err)
		}
	}
	var fields struct {
		Offset *toolarg.Number `json:"offset"`
		Limit  toolarg.Number  `json:"limit"`
	}
	if err := json.Unmarshal([]byte(`{"offset":null,"limit":"7"}`), &fields); err != nil || fields.Offset != nil || fields.Limit != 7 {
		t.Fatalf("fields = %+v, %v", fields, err)
	}
}

func TestCountWantsAWholeNumberNotBelowZero(t *testing.T) {
	number := func(v float64) *toolarg.Number { n := toolarg.Number(v); return &n }
	if got, err := toolarg.Count("offset", nil); got != 0 || err != nil {
		t.Fatalf("nil: %d, %v", got, err)
	}
	if got, err := toolarg.Count("offset", number(1860)); got != 1860 || err != nil {
		t.Fatalf("1860: %d, %v", got, err)
	}
	if _, err := toolarg.Count("offset", number(1.5)); err == nil || !strings.Contains(err.Error(), `"offset" must be a whole number`) {
		t.Fatalf("1.5: %v", err)
	}
	if _, err := toolarg.Count("limit", number(-1)); err == nil || !strings.Contains(err.Error(), `"limit" must not be negative`) {
		t.Fatalf("-1: %v", err)
	}
	if got := toolarg.Number(2.5).Rounded(); got != 3 {
		t.Fatalf("rounded = %d", got)
	}
}
