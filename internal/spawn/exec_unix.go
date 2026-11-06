//go:build unix && !darwin

package spawn

import "syscall"

// Exec replaces this process, preserving descriptors not marked close-on-exec.
func Exec(path string, argv, env []string) error {
	return syscall.Exec(path, argv, env)
}
