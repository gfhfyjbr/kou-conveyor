package localfile

import (
	"os"
	"syscall"
	"unsafe"

	"golang.org/x/sys/unix"
)

// filePage is what mincore says of a page that is the file's own, in memory
// or not, looked at or not: MINCORE_INCORE, MINCORE_REFERENCED and
// MINCORE_REFERENCED_OTHER. A page written to, the process's own copy, is
// also MINCORE_MODIFIED, MINCORE_COPIED and MINCORE_ANONYMOUS.
const filePage = 0x1 | 0x2 | 0x8

// dropPages maps a file again over its mapping, which takes the mapping's
// pages out of the process's resident memory: the system keeps them as the
// file's cache, and the mapping reads them again as they are used. On darwin
// no madvise does that for the pages of a file. A page written to is the
// process's own copy rather than the file's: it keeps its mapping, and what
// was written to it.
func dropPages(file *os.File, mapped []byte) {
	page := os.Getpagesize()
	pages := make([]byte, (len(mapped)+page-1)/page)
	start := unsafe.Pointer(unsafe.SliceData(mapped))
	if _, _, errno := syscall.Syscall(
		syscall.SYS_MINCORE,
		uintptr(start), uintptr(len(mapped)), uintptr(unsafe.Pointer(unsafe.SliceData(pages))),
	); errno != 0 {
		return
	}
	first := -1
	for index := 0; index <= len(pages); index++ {
		own := index < len(pages) && pages[index]&^filePage == 0
		if own && first < 0 {
			first = index
		}
		if !own && first >= 0 {
			offset := first * page
			// A mapping that cannot be replaced is left as it was.
			_, _ = unix.MmapPtr(
				int(file.Fd()), int64(offset), unsafe.Add(start, offset),
				uintptr(min(index*page, len(mapped))-offset),
				unix.PROT_READ|unix.PROT_WRITE, unix.MAP_PRIVATE|unix.MAP_FIXED,
			)
			first = -1
		}
	}
}
