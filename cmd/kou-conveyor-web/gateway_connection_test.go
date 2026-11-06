//go:build darwin || linux || freebsd || openbsd || netbsd || dragonfly

package main

import (
	"net/http"
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
)

func TestGatewayFrontendConnectionSelection(t *testing.T) {
	for _, mode := range []string{"managed", "off"} {
		t.Run(mode, func(t *testing.T) {
			o := shutdownFixture(t)
			address := o.accounts
			if mode == "off" {
				address = "off"
			}
			front := startShutdownServer(t, o.Workspace, address)
			var host gatewayHostDescription
			if mode == "managed" {
				var err error
				host, err = loadGatewayHost(o.accountsDir)
				if err != nil {
					t.Fatal(err)
				}
			}
			t.Cleanup(func() {
				front.interrupt(t)
				if mode == "managed" {
					waitGatewayGone(t, newGatewayPeer(host, nil))
				}
			})
			var answer struct {
				Connection connectionView `json:"connection"`
				Gateway    gatewayView    `json:"gateway"`
			}
			shutdownJSON(t, front, "PUT", "/api/connection", `{"mode":"environment"}`, http.StatusOK, &answer)
			if answer.Connection.Mode != "environment" {
				t.Fatalf("environment selection: %+v", answer)
			}
			choice := `{"mode":"gateway","model":"fake-model","protocol":"messages"}`
			if mode == "off" {
				shutdownJSON(t, front, "PUT", "/api/connection", choice, http.StatusConflict, nil)
				saved, err := cockpit.LoadSettings(o.SettingsFile)
				if err != nil || saved != (cockpit.Settings{}) {
					t.Fatalf("a rejected selection changed local settings: %+v %v", saved, err)
				}
				return
			}
			shutdownJSON(t, front, "PUT", "/api/connection", choice, http.StatusOK, &answer)
			want := cockpit.Settings{API: cockpit.APIGateway, Model: "fake-model", Protocol: cockpit.APIMessages}
			saved, err := cockpit.LoadSettings(o.SettingsFile)
			if err != nil || saved != want {
				t.Fatalf("gateway selection was not saved by this front-end: %+v %v", saved, err)
			}
			if answer.Connection.Mode != "gateway" || answer.Connection.Model != want.Model || answer.Connection.Protocol != want.Protocol || answer.Connection.API != want.Protocol || !answer.Gateway.Enabled || answer.Gateway.Use == nil || answer.Gateway.Use.API != want.Protocol {
				t.Fatalf("gateway selection envelope: %+v", answer)
			}
			shutdownJSON(t, front, "GET", "/api/accounts", "", http.StatusOK, &answer)
			if answer.Connection.Mode != "gateway" || answer.Connection.Model != want.Model || answer.Connection.Protocol != want.Protocol || !answer.Gateway.Enabled {
				t.Fatalf("the accounts listing lost the front-end's selection: %+v", answer)
			}
		})
	}
}
