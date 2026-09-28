package main

import (
	"cmp"
	"fmt"
	"net/http"
	"strconv"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/accounts"
)

// What the gateway's requests used, model by model, and what that comes to
// at API prices — as API credits — which the Accounts view's Usage tab
// shows; and the prices tokens are counted at, which the user can set.

// handleUsage reports on the requests of ?range= (24h, 7d, 30d or 90d),
// in the browser's time zone (?tz=, else ?offset= in minutes east of UTC),
// narrowed to one ?provider=, ?account= or ?model= if asked.
func (s *server) handleUsage(w http.ResponseWriter, r *http.Request, gateway *accounts.Host) {
	query := r.URL.Query()
	span := cmp.Or(query.Get("range"), "7d")
	if _, ok := accounts.UsageRanges[span]; !ok {
		writeError(w, http.StatusBadRequest, "range is 24h, 7d, 30d or 90d")
		return
	}
	report, err := gateway.Usage(r.Context(), accounts.UsageQuery{
		Range: span, Location: zoneOf(query.Get("tz"), query.Get("offset")),
		Filter: accounts.UsageFilter{Provider: query.Get("provider"), Account: query.Get("account"), Model: query.Get("model")},
	})
	if err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	writeJSON(w, http.StatusOK, report)
}

// zoneOf is the browser's time zone: by its name, else by its offset from
// UTC in minutes, else UTC.
func zoneOf(name, offset string) *time.Location {
	if name != "" && len(name) <= 64 && name != "Local" {
		if loc, err := time.LoadLocation(name); err == nil {
			return loc
		}
	}
	if minutes, err := strconv.Atoi(offset); err == nil && minutes != 0 && minutes >= -14*60 && minutes <= 14*60 {
		sign, abs := '+', minutes
		if minutes < 0 {
			sign, abs = '-', -minutes
		}
		return time.FixedZone(fmt.Sprintf("UTC%c%02d:%02d", sign, abs/60, abs%60), minutes*60)
	}
	return time.UTC
}

// handlePrices lists the prices tokens are counted at: the user's, those
// the cockpit knows, and every model with the price it gets.
func (s *server) handlePrices(w http.ResponseWriter, r *http.Request, gateway *accounts.Host) {
	writeJSON(w, http.StatusOK, gateway.Prices(r.Context()))
}

// handleSetPrice sets the price of a model, or of the models a pattern
// matches: {match, input, output, cache_read, cache_write} in US dollars
// per million tokens.
func (s *server) handleSetPrice(w http.ResponseWriter, r *http.Request, gateway *accounts.Host) {
	var rule accounts.PriceRule
	if !decodeJSON(w, r, &rule) {
		return
	}
	if _, err := gateway.SetPrice(rule); err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	writeJSON(w, http.StatusOK, gateway.Prices(r.Context()))
}

// handleRemovePrice drops the user's price of ?match=: its models go back
// to the prices the cockpit knows.
func (s *server) handleRemovePrice(w http.ResponseWriter, r *http.Request, gateway *accounts.Host) {
	if err := gateway.RemovePrice(r.URL.Query().Get("match")); err != nil {
		writeError(w, http.StatusNotFound, err.Error())
		return
	}
	writeJSON(w, http.StatusOK, gateway.Prices(r.Context()))
}

// handleSetUnit chooses what amounts are shown in: {name: "USD"}, or
// credits of the user's, {name, usd} with what one is worth in dollars.
func (s *server) handleSetUnit(w http.ResponseWriter, r *http.Request, gateway *accounts.Host) {
	var unit accounts.Unit
	if !decodeJSON(w, r, &unit) {
		return
	}
	if _, err := gateway.SetUnit(unit); err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	writeJSON(w, http.StatusOK, gateway.Prices(r.Context()))
}

// handleRefreshPrices fetches OpenRouter's prices now.
func (s *server) handleRefreshPrices(w http.ResponseWriter, r *http.Request, gateway *accounts.Host) {
	if err := gateway.RefreshPrices(r.Context()); err != nil {
		writeError(w, http.StatusBadGateway, err.Error())
		return
	}
	writeJSON(w, http.StatusOK, gateway.Prices(r.Context()))
}
