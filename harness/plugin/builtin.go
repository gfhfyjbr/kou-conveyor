package plugin

import (
	"embed"
	"io/fs"
	"sync"
)

// The harness's built-in plugins besides core are directories compiled into
// it (builtin/), read as any plugin's directory is: guide gives the agent
// kou-conveyor's own skills, such as how to write plugins for it.
//
//go:embed builtin
var builtinFiles embed.FS

var compiled struct {
	once     sync.Once
	plugins  []Plugin
	problems []error
}

// compiledBuiltins reads the built-in plugins compiled in as directories,
// once: their files never change while the program runs.
func compiledBuiltins() ([]Plugin, []error) {
	compiled.once.Do(func() {
		sub, err := fs.Sub(builtinFiles, "builtin")
		if err != nil {
			compiled.problems = []error{err}
			return
		}
		compiled.plugins, compiled.problems = ReadDirectoryFS(sub, SourceBuiltin)
	})
	return compiled.plugins, compiled.problems
}
