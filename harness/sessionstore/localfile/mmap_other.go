//go:build !unix

package localfile

import "os"

// mapFile reads a file: where files are not mapped, it never maps one.
func mapFile(path string) ([]byte, *mapping, error) {
	encoded, err := os.ReadFile(path)
	return encoded, nil, err
}
