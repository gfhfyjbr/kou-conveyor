//go:build !darwin

package localfile

import "os"

// dropPages leaves a mapping's pages resident: only on darwin does the store
// know which pages are the process's own copies, which must stay.
func dropPages(*os.File, []byte) {}
