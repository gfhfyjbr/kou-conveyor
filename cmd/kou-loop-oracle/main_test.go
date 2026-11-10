package main

import (
	"encoding/json/v2"
	"strings"
	"testing"
)

func TestActualCoordinatorScripts(t *testing.T) {
	const input = `{"Kind":"input","Input":{"ID":"one","Kind":"external","Payload":"owned task"}}`
	const request = `{"Kind":"request"}`
	const stopIdle = `{"Kind":"input","Input":{"ID":"stop","Kind":"control","Payload":{"Mode":"when_idle"}}},{"Kind":"wait_input","ID":"stop"}`
	const stopHard = `{"Kind":"input","Input":{"ID":"stop","Kind":"control","Payload":{"Mode":"hard","Reason":"requested stop"}}},{"Kind":"cancel_op","Count":1}`
	const finish = `{"Kind":"finish"}`
	const answer0 = `{"Kind":"respond","Index":0,"Response":{"Output":[{"Type":"message","Data":{"Role":"assistant","Text":"done"}}]}}`
	const answer1 = `{"Kind":"respond","Index":1,"Response":{"Output":[{"Type":"message","Data":{"Role":"assistant","Text":"done"}}]}}`
	const call = `{"Kind":"respond","Index":0,"Response":{"Output":[{"Type":"tool_call","Data":{"CallID":"c1","Name":"Bash","Arguments":"1"}}]}}`
	const compact = `{"Kind":"input","Input":{"ID":"compact","Kind":"control","Payload":{"Mode":"compact"}}}`
	cases := []struct {
		name                                       string
		steps                                      []string
		requests, dispatches, cancels, checkpoints int
		failure                                    string
	}{
		{"plain", []string{input, request, stopIdle, answer0, finish}, 1, 0, 0, 0, ""},
		{"completed-operation", []string{input, request, call, `{"Kind":"add"}`, `{"Kind":"complete","Index":0}`, request, stopIdle, answer1, finish}, 2, 1, 0, 0, ""},
		{"hard-stop", []string{input, request, call, `{"Kind":"add"}`, stopHard, `{"Kind":"complete","Index":0,"Status":"canceled"}`, finish}, 1, 1, 1, 0, ""},
		{"failed-status", []string{input, request, `{"Kind":"fail","Fail":"status"}`, call, finish}, 1, 0, 0, 0, `store tool call "c1" status: status failed`},
		{"failed-summary", []string{input, request, answer0, compact, request, stopIdle, `{"Kind":"respond","Index":1,"Response":{"Output":[{"Type":"message","Data":{"Role":"assistant","Text":"<review>only</review>"}}]}}`, finish}, 2, 0, 0, 0, ""},
		{"successful-summary", []string{input, request, answer0, compact, request, stopIdle, `{"Kind":"respond","Index":1,"Response":{"Output":[{"Type":"message","Data":{"Role":"assistant","Text":"<summary>kept task</summary>"}}]}}`, finish}, 2, 0, 0, 1, ""},
	}
	for _, fixture := range cases {
		t.Run(fixture.name, func(t *testing.T) {
			var input spec
			if err := json.Unmarshal([]byte(`{"Actions":[`+strings.Join(fixture.steps, ",")+`]}`), &input); err != nil {
				t.Fatal(err)
			}
			output, err := run(input)
			if err != nil {
				t.Fatal(err)
			}
			encoded, err := json.Marshal(output)
			if err != nil {
				t.Fatal(err)
			}
			var result struct {
				Trace []entry
				Error string
			}
			if err := json.Unmarshal(encoded, &result); err != nil {
				t.Fatal(err)
			}
			if result.Error != fixture.failure {
				t.Fatalf("error %q; want %q", result.Error, fixture.failure)
			}
			counts := map[string]int{}
			for _, entry := range result.Trace {
				counts[entry.Kind]++
			}
			for kind, want := range map[string]int{"request": fixture.requests, "dispatch": fixture.dispatches, "cancel": fixture.cancels, "checkpoint": fixture.checkpoints} {
				if counts[kind] != want {
					t.Fatalf("%s: got %d want %d; trace %s", kind, counts[kind], want, encoded)
				}
			}
		})
	}
}
