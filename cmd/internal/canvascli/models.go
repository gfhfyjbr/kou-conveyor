package canvascli

import (
	"context"
	"fmt"
	"net/url"
	"strings"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
)

type listModelsArgs struct {
	Fresh loose `json:"fresh"`
}

func (c *client) listModels(ctx context.Context, a listModelsArgs) (result, error) {
	ctx, cancel := context.WithTimeout(ctx, 30*time.Second)
	defer cancel()
	query := url.Values{}
	if a.Fresh {
		query.Set("fresh", "1")
	}
	var catalog struct {
		cockpit.Catalog
		Omitted int `json:"omitted"`
	}
	raw, err := c.call(ctx, "GET", "/api/canvas/models", query, nil, &catalog)
	if err != nil {
		return result{}, err
	}
	return result{text: formatModels(catalog.Catalog, catalog.Omitted), raw: raw}, nil
}

func formatModels(catalog cockpit.Catalog, omitted int) string {
	var b strings.Builder
	b.WriteString("Models for CanvasSpawn config.model")
	if catalog.Source != "" {
		fmt.Fprintf(&b, " (source: %s)", catalog.Source)
	}
	b.WriteString("\nUse an entry's id unchanged, not its display name; IDs include aliases and endpoint prefixes.\n")
	if catalog.Default != "" {
		fmt.Fprintf(&b, "New-session default when model is omitted: %s", catalog.Default)
		if _, found := catalog.Find(catalog.Default); !found {
			b.WriteString(" (not listed; availability unverified)")
		}
		b.WriteByte('\n')
	}
	if catalog.Error != "" {
		fmt.Fprintf(&b, "Catalog unavailable: %s\n", catalog.Error)
	}
	if omitted > 0 {
		fmt.Fprintf(&b, "Omitted %d catalog entries with invalid model IDs; listed IDs are unchanged.\n", omitted)
	}
	if len(catalog.Models) == 0 {
		if catalog.Source == "" && catalog.Error == "" {
			b.WriteString("This connection does not provide a model catalog.\n")
		} else {
			b.WriteString("No agent models are listed.\n")
		}
	}
	for _, model := range catalog.Models {
		fmt.Fprintf(&b, "  id: %s", model.ID)
		if model.Name != "" {
			fmt.Fprintf(&b, " | name: %s", model.Name)
		}
		provider := model.ProviderName
		if provider == "" {
			provider = model.Provider
		}
		if provider != "" {
			fmt.Fprintf(&b, " | provider: %s", provider)
		}
		if len(model.Via) > 0 {
			fmt.Fprintf(&b, " | via: %s", strings.Join(model.Via, ", "))
		}
		if model.API != "" {
			fmt.Fprintf(&b, " | API: %s", model.API)
		}
		if model.Context > 0 {
			fmt.Fprintf(&b, " | context: %d tokens", model.Context)
		}
		if model.Cooling {
			b.WriteString(" | cooling: registered, but all routes are rate-limited")
		}
		b.WriteByte('\n')
	}
	b.WriteString("Media-only models are excluded. Per-model effort and other capability support are not supplied by this catalog; missing metadata means unknown.\n")
	return b.String()
}
