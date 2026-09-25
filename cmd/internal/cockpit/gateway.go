package cockpit

import (
	"context"
	"encoding/json/v2"
	"errors"
	"fmt"
	"io/fs"
	"net"
	"net/url"
	"os"
	"path/filepath"
	"strings"
	"time"
)

// Gateway is the accounts gateway the browser cockpit runs (package
// accounts): CLIProxyAPI, serving the Responses and Messages APIs from the
// accounts and endpoints added to it. The cockpit that runs it publishes
// where it listens and its key in gateway.json beside the settings, readable
// only by the user; settings of APIGateway resolve against it as a run
// starts, in whichever cockpit starts the run.
type Gateway struct {
	URL    string `json:"url"`
	APIKey string `json:"api_key"`
}

// ErrGatewayOff reports that no gateway runs.
var ErrGatewayOff = errors.New("the accounts gateway is not running: kou-conveyor-web runs it")

// GatewayPath is the gateway's publication that goes with a settings file,
// or "" when there is none.
func GatewayPath(settingsFile string) string {
	if settingsFile == "" {
		return ""
	}
	return filepath.Join(filepath.Dir(settingsFile), "gateway.json")
}

// PublishGateway records where the gateway runs.
func PublishGateway(path string, g Gateway) error {
	data, err := json.Marshal(g, json.Deterministic(true))
	if err != nil {
		return err
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		return err
	}
	return replaceFile(path, append(data, '\n'))
}

// WithdrawGateway removes the publication, if it still names g: another
// cockpit may have published its own since.
func WithdrawGateway(path string, g Gateway) {
	if current, err := LoadGateway(path); err == nil && current == g {
		_ = os.Remove(path)
	}
}

// LoadGateway reads the publication; ErrGatewayOff when there is none.
func LoadGateway(path string) (Gateway, error) {
	if path == "" {
		return Gateway{}, ErrGatewayOff
	}
	data, err := os.ReadFile(path)
	if errors.Is(err, fs.ErrNotExist) {
		return Gateway{}, ErrGatewayOff
	}
	if err != nil {
		return Gateway{}, err
	}
	var g Gateway
	if err := json.Unmarshal(data, &g); err != nil || g.URL == "" || g.APIKey == "" {
		return Gateway{}, fmt.Errorf("%s is not a gateway's publication", path)
	}
	return g, nil
}

// Reach checks that the gateway answers, so a run that cannot reach it
// says so before it starts rather than at its first request.
func (g Gateway) Reach(ctx context.Context) error {
	u, err := url.Parse(g.URL)
	if err != nil || u.Host == "" {
		return fmt.Errorf("the gateway's address %q is not a URL", g.URL)
	}
	dialer := net.Dialer{Timeout: 2 * time.Second}
	conn, err := dialer.DialContext(ctx, "tcp", u.Host)
	if err != nil {
		return fmt.Errorf("%w: nothing answers at %s", ErrGatewayOff, g.URL)
	}
	return conn.Close()
}

// GatewayAPI is the API runs speak through the gateway for a model: Claude
// models the Messages API, which they speak natively; the others the
// Responses API, which the gateway translates for providers that speak
// another.
func GatewayAPI(model string) API {
	name := strings.ToLower(model)
	if i := strings.LastIndexByte(name, '/'); i >= 0 {
		name = name[i+1:]
	}
	if strings.HasPrefix(name, "claude") {
		return APIMessages
	}
	return APIResponses
}

// ForModel is the connection a run with model uses: these settings with
// model in place of their own, "" keeping theirs. Through the gateway the API
// follows the model, since a session may go from one provider's model to
// another's: a protocol chosen for the settings' model stays with it alone.
func (s Settings) ForModel(model string) Settings {
	model = strings.TrimSpace(model)
	// The environment's connection takes the model in the request alone.
	if model == "" || model == s.Model || s.API == APIEnvironment {
		return s
	}
	s.Model = model
	if s.API == APIGateway {
		s.Protocol = ""
	}
	return s
}

// Through resolves settings that send runs through the gateway into the
// endpoint the gateway listens on and its key; other settings are returned
// as they are. The Messages API's clients add /v1 to a base URL themselves,
// the Responses API's do not.
func (s Settings) Through(g Gateway) Settings {
	if s.API != APIGateway {
		return s
	}
	api := s.Protocol
	if api == "" {
		api = GatewayAPI(s.Model)
	}
	base := strings.TrimRight(g.URL, "/")
	if api == APIResponses {
		base += "/v1"
	}
	return Settings{API: api, BaseURL: base, APIKey: g.APIKey, Model: s.Model}
}

// ResolveGateway resolves settings of APIGateway against the gateway
// published beside the settings file, once it answers.
func ResolveGateway(ctx context.Context, settingsFile string, s Settings) (Settings, error) {
	if s.API != APIGateway {
		return s, nil
	}
	g, err := LoadGateway(GatewayPath(settingsFile))
	if err != nil {
		return s, err
	}
	if err := g.Reach(ctx); err != nil {
		return s, err
	}
	return s.Through(g), nil
}
