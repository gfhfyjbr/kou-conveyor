package inbox_test

import (
	"encoding/json/jsontext"
	"encoding/json/v2"
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/harness/inbox"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
)

func TestInboxControlMessages(t *testing.T) {
	for _, mode := range []inbox.ControlMode{inbox.StopHard, inbox.StopWhenIdle, inbox.Heartbeat, inbox.Compact} {
		t.Run(string(mode), func(t *testing.T) {
			want := inbox.ControlMessage{Mode: mode, Reason: "stop now"}
			payload, err := json.Marshal(want)
			if err != nil {
				t.Fatal(err)
			}
			input := inbox.Input{ID: "stop", Kind: inbox.InputControl, Payload: payload}
			got, err := submitAndReceive(t, newInbox(t), input).DecodeControlMessage()
			if err != nil || got != want {
				t.Fatalf("control message = %+v, error = %v", got, err)
			}
		})
	}
}

func TestInboxSettingsControls(t *testing.T) {
	for _, effort := range []llm.ReasoningEffort{llm.ReasoningEffortLow, llm.ReasoningEffortMedium, llm.ReasoningEffortHigh, llm.ReasoningEffortXHigh, llm.ReasoningEffortMax} {
		t.Run(string(effort), func(t *testing.T) {
			want := inbox.ControlMessage{Mode: inbox.UpdateSettings, Parameters: inbox.Settings{ReasoningEffort: effort}}
			payload, err := json.Marshal(want)
			if err != nil {
				t.Fatal(err)
			}
			input := inbox.Input{ID: "settings", Kind: inbox.InputControl, Payload: payload}
			got, err := submitAndReceive(t, newInbox(t), input).DecodeControlMessage()
			if err != nil || got != want {
				t.Fatalf("settings control = %+v, error = %v", got, err)
			}
		})
	}
}

func TestInboxRejectsInvalidControlMessages(t *testing.T) {
	for _, payload := range []string{
		"", "null", `{}`, `{"Mode":"unknown"}`, `{"Mode":"soft"}`, `{"Mode":42}`, `{"Mode":"hard"`,
		`{"Mode":"hard","extra":true}`,
		`{"Mode":"heartbeat","Reason":"waiting","extra":true}`,
		`{"Mode":"heartbeat"}`,
		`{"Mode":"heartbeat","Reason":""}`,
		`{"Mode":"heartbeat","Reason":null}`,
		`{"Mode":"settings"}`,
		`{"Mode":"settings","Parameters":null}`,
		`{"Mode":"settings","Parameters":{}}`,
		`{"Mode":"settings","Parameters":[]}`,
		`{"Mode":"settings","Parameters":"high"}`,
		`{"Mode":"settings","Parameters":{"ReasoningEffort":""}}`,
		`{"Mode":"settings","Parameters":{"ReasoningEffort":null}}`,
		`{"Mode":"settings","Parameters":{"ReasoningEffort":"default"}}`,
		`{"Mode":"settings","Parameters":{"ReasoningEffort":"turbo"}}`,
		`{"Mode":"settings","Parameters":{"ReasoningEffort":42}}`,
		`{"Mode":"settings","Parameters":{"Model":"model"}}`,
		`{"Mode":"settings","Parameters":{"Model":"model","ReasoningEffort":"high"}}`,
		`{"Mode":"settings","Parameters":{"ReasoningEffort":"high","extra":true}}`,
		`{"Mode":"settings","Parameters":{"ReasoningEffort":"high"},"extra":true}`,
		`{"Mode":"hard","Parameters":{"ReasoningEffort":"high"}}`,
		`{"Mode":"when_idle","Parameters":{"ReasoningEffort":"high"}}`,
		`{"Mode":"heartbeat","Reason":"waiting","Parameters":{"ReasoningEffort":"high"}}`,
		`{"Mode":"hard","Parameters":null}`,
		`{"Mode":"compact","Parameters":{"ReasoningEffort":"high"}}`,
		`{"Mode":"compact","Reason":"focus","extra":true}`,
	} {
		t.Run(payload, func(t *testing.T) {
			input := inbox.Input{ID: "stop", Kind: inbox.InputControl, Payload: jsontext.Value(payload)}
			if err := newInbox(t).Submit(t.Context(), input); err == nil {
				t.Fatal("invalid control message accepted")
			}
		})
	}
	if _, err := (inbox.Input{Kind: inbox.InputExternal, Payload: jsontext.Value(`{"Mode":"hard"}`)}).DecodeControlMessage(); err == nil {
		t.Fatal("external input decoded as control message")
	}
}

func TestInboxDeliveryAfterTools(t *testing.T) {
	payload, err := json.Marshal("use pnpm")
	if err != nil {
		t.Fatal(err)
	}
	input := inbox.Input{ID: "steer", Kind: inbox.InputExternal, Payload: payload, Delivery: inbox.DeliverAfterTools}
	if got := submitAndReceive(t, newInbox(t), input); got.Delivery != inbox.DeliverAfterTools {
		t.Fatalf("delivery = %q", got.Delivery)
	}
	control, err := json.Marshal(inbox.ControlMessage{Mode: inbox.StopHard})
	if err != nil {
		t.Fatal(err)
	}
	for name, invalid := range map[string]inbox.Input{
		"control":    {ID: "stop", Kind: inbox.InputControl, Payload: control, Delivery: inbox.DeliverAfterTools},
		"unknown":    {ID: "steer", Kind: inbox.InputExternal, Payload: payload, Delivery: "later"},
		"crash mode": {ID: "crash", Kind: inbox.InputCrash, Delivery: inbox.DeliverAfterTools},
	} {
		if err := invalid.Validate(); err == nil {
			t.Errorf("%s: delivery %q was accepted", name, invalid.Delivery)
		}
	}
	// Recorded input keeps its delivery, and sessions without it still read.
	encoded, err := json.Marshal(input)
	if err != nil {
		t.Fatal(err)
	}
	var decoded inbox.Input
	if err := json.Unmarshal(encoded, &decoded); err != nil || decoded.Delivery != inbox.DeliverAfterTools {
		t.Fatalf("decoded %s as %+v, %v", encoded, decoded, err)
	}
	var old inbox.Input
	if err := json.Unmarshal([]byte(`{"ID":"old","Kind":"external","Payload":"hi"}`), &old); err != nil || old.Delivery != inbox.DeliverAtOnce {
		t.Fatalf("an input without delivery decoded as %+v, %v", old, err)
	}
}
