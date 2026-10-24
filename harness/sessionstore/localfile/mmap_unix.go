//go:build unix

package localfile

import (
	"io"
	"os"
	"syscall"
)

// mapFile maps a file into memory, or reads it where it cannot, as an empty
// file. The mapping comes with the file, open (mapping), nil for a file
// read.
//
// The mapping is private: a write to it, as of a value decoded into in
// place, goes to a copy of the page, as a write to memory read from the file
// would, and never to the file. It is never unmapped, as what was decoded
// from it refers to it for as long as the process keeps that, which nothing
// tracks. It stays valid as the file grows, and as it is replaced, since a
// store replaces a file by a new one (publishFile) and cuts one only of the
// part past its last record, which nothing decoded refers to.
func mapFile(path string) (encoded []byte, mapped *mapping, err error) {
	file, err := os.Open(path)
	if err != nil {
		return nil, nil, err
	}
	info, err := file.Stat()
	if err != nil {
		file.Close()
		return nil, nil, err
	}
	size := info.Size()
	if size > 0 && int64(int(size)) == size {
		bytes, err := syscall.Mmap(
			int(file.Fd()), 0, int(size),
			syscall.PROT_READ|syscall.PROT_WRITE, syscall.MAP_PRIVATE,
		)
		if err == nil {
			return bytes, &mapping{file: file, bytes: bytes}, nil
		}
	}
	defer file.Close()
	encoded, err = io.ReadAll(file)
	return encoded, nil, err
}
