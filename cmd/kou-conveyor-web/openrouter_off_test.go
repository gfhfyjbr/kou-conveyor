package main

import "os"

// The tests never ask OpenRouter for its prices.
func init() { os.Setenv("KOU_CONVEYOR_OPENROUTER_PRICES", "off") }
