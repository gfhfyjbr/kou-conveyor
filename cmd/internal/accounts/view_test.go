package accounts

import (
	"testing"
	"time"
)

func TestAccountStates(t *testing.T) {
	now := time.Date(2026, 9, 24, 12, 0, 0, 0, time.UTC)
	later := now.Add(10 * time.Minute).Format(time.RFC3339)
	for _, c := range []struct {
		name, entry, state string
	}{
		{"ready", `{"status":"active","status_message":"ok"}`, StateReady},
		{"disabled", `{"status":"active","disabled":true}`, StateDisabled},
		{"refreshing", `{"status":"refreshing"}`, StateRefreshing},
		{"cooling", `{"status":"error","unavailable":true,"next_retry_after":"` + later + `"}`, StateCooling},
		{"cooling account-wide", `{"status":"error","cooldowns":[{"scope":"credential","reason":"quota","retry_at":"` + later + `"}]}`, StateCooling},
		{"a cooling model", `{"status":"active","cooldowns":[{"scope":"model","model_key":"gpt-5","reason":"quota","retry_at":"` + later + `"}]}`, StateReady},
		{"failing", `{"status":"error","unavailable":true,"status_message":"unauthorized"}`, StateError},
	} {
		a := accountOf(decode(t, `{"name":"x.json","provider":"codex","auth_index":"1",`+c.entry[1:]), now)
		if a.State != c.state {
			t.Errorf("%s: state = %q, want %q", c.name, a.State, c.state)
		}
	}
	a := accountOf(decode(t, `{"name":"codex-x.json","id":"codex-x.json","auth_index":"abc","provider":"codex","email":"me@example.com",
		"account_type":"oauth","status":"active","id_token":{"plan_type":"plus","chatgpt_account_id":"acc-1"},
		"cooldowns":[{"scope":"model","model_key":"gpt-5","reason":"quota","retry_at":"`+later+`","http_status":429}]}`), now)
	if a.Label != "me@example.com" || a.Plan != "Plus" || a.ProviderName != "Codex" || !a.QuotaSupported || a.target.chatgptAccount != "acc-1" {
		t.Errorf("account = %+v", a)
	}
	if len(a.Cooldowns) != 1 || a.Cooldowns[0].Model != "gpt-5" || a.Cooldowns[0].Status != 429 {
		t.Errorf("cooldowns = %+v", a.Cooldowns)
	}
	key := accountOf(decode(t, `{"name":"claude-key","provider":"claude","account_type":"api_key","account":"sk-ant-1234567890abcdef","runtime_only":true,"auth_index":"2"}`), now)
	if key.Label != "sk-a…cdef" || !key.Config || key.QuotaSupported {
		t.Errorf("a key is masked, and its limits are not asked for: %+v", key)
	}
}
