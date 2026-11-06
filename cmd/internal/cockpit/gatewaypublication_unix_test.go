//go:build darwin || linux || freebsd || openbsd || netbsd || dragonfly

package cockpit

import (
	"encoding/json/v2"
	"path/filepath"
	"testing"
	"time"
)

func TestGatewayPublicationCoordinatesPublishAndWithdraw(t *testing.T) {
	path := filepath.Join(t.TempDir(), "gateway.json")
	first := Gateway{URL: "http://127.0.0.1:8318", APIKey: "key", Owner: "first"}
	second := first
	second.Owner = "second"
	if err := PublishGateway(path, first); err != nil {
		t.Fatal(err)
	}
	unlock, err := lockGatewayPublication(path)
	if err != nil {
		t.Fatal(err)
	}
	defer func() {
		if unlock != nil {
			unlock()
		}
	}()
	published := make(chan error, 1)
	go func() { published <- PublishGateway(path, second) }()
	select {
	case err := <-published:
		t.Fatalf("PublishGateway ignored the publication's ownership lock: %v", err)
	case <-time.After(75 * time.Millisecond):
	}
	unlock()
	unlock = nil
	if err := <-published; err != nil {
		t.Fatal(err)
	}
	unlock, err = lockGatewayPublication(path)
	if err != nil {
		t.Fatal(err)
	}
	withdrawn := make(chan struct{})
	go func() { WithdrawGateway(path, second); close(withdrawn) }()
	select {
	case <-withdrawn:
		t.Fatal("WithdrawGateway ignored the publication's ownership lock")
	case <-time.After(75 * time.Millisecond):
	}
	// This write owns the publication lock. The pending old withdrawal must
	// re-read the generation only after it obtains the same lock.
	third := second
	third.Owner = "third"
	data, _ := json.Marshal(third)
	if err := replaceFile(path, data); err != nil {
		t.Fatal(err)
	}
	unlock()
	unlock = nil
	<-withdrawn
	current, err := LoadGateway(path)
	if err != nil || current != third {
		t.Fatalf("stale withdrawal removed the successor: %+v %v", current, err)
	}
}
