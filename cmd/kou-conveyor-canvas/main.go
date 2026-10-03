// Command kou-conveyor-canvas is kou-canvas: how the programs of a
// canvas's nodes see the canvas and act on it (see canvascli). The server
// puts it on the nodes' PATH as kou-canvas.
package main

import (
	"os"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/canvascli"
)

func main() { os.Exit(canvascli.Main(os.Args[1:])) }
