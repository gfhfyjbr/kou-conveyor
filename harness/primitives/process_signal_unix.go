//go:build unix && !darwin

package primitives

func normalizeProcessGroupSignalError(_ int, err error) error {
	return err
}
