package main

import (
	"encoding/json/v2"
	"fmt"
	"os"
	"path/filepath"

	"github.com/gfhfyjbr/kou-conveyor/harness/llm/clients/openaicodex"
)

// This action validates synthetic credentials only. It never calls Respond,
// accepts an auth-file path, or reads the process environment.
func runCodexConfig(data []byte) error {
	var input struct {
		Codex struct {
			Mode                            string
			Environment                     map[string]string
			AccessToken, AccountID, BaseURL string
			MaxAttempts                     *int
			AuthBody                        *string
			FileMode                        *uint32
		}
	}
	if err := json.Unmarshal(data, &input); err != nil {
		return err
	}
	var result any
	if input.Codex.Mode == "environment" {
		if input.Codex.Environment["HOME"] == "" {
			return fmt.Errorf("oracle environment needs a synthetic HOME")
		}
		config, err := openaicodex.EnvironmentConfig(func(name string) string { return input.Codex.Environment[name] })
		var view any
		message := ""
		if err != nil {
			message = err.Error()
		} else {
			view = struct{ AccessToken, AccountID, AuthFile string }{config.AccessToken, config.AccountID, config.AuthFile}
		}
		result = struct {
			Config any
			Error  string
		}{view, message}
	} else {
		config := openaicodex.Config{AccessToken: input.Codex.AccessToken, AccountID: input.Codex.AccountID, BaseURL: input.Codex.BaseURL, MaxAttempts: input.Codex.MaxAttempts}
		if input.Codex.AuthBody != nil {
			directory, err := os.MkdirTemp("", "kou-codex-oracle-")
			if err != nil {
				return err
			}
			defer os.RemoveAll(directory)
			config.AuthFile = filepath.Join(directory, "auth.json")
			if err := os.WriteFile(config.AuthFile, []byte(*input.Codex.AuthBody), 0600); err != nil {
				return err
			}
			if input.Codex.FileMode != nil {
				if err := os.Chmod(config.AuthFile, os.FileMode(*input.Codex.FileMode&0777)); err != nil {
					return err
				}
			}
		}
		client, err := openaicodex.NewClient(config)
		message := ""
		if err != nil {
			message = err.Error()
		} else {
			defer client.Close()
		}
		result = struct{ Error string }{message}
	}
	encoded, err := json.Marshal(struct {
		Result any
		Error  string
	}{result, ""})
	if err != nil {
		return err
	}
	_, err = os.Stdout.Write(append(encoded, '\n'))
	return err
}
