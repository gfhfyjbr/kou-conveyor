package localfile

import "os"

// A mapping is a session file mapped into memory (mapFile), with the file,
// kept open to map it again.
//
// Decoding reads the whole mapping, and the pages it read count in the
// process's resident memory, as ps and top show it, though they are the
// file's and what was decoded needs few of them: on a long session,
// hundreds of megabytes. drop gives them back, and what was decoded reads
// them from the file again as it needs them. A store drops a mapping's pages
// once it has decoded it, and again as it lets go of what it decoded, which
// was read through meanwhile: a run restoring its session reads the outputs
// of all its tool calls.
//
// A page written to, as of a value decoded into in place, is the process's
// own copy, which drop keeps. A page written to for the first time as drop
// gives the pages back may lose the write: nothing a store decoded is
// written to in place while it holds it, and what it lets go of is to be
// written to, if at all, by what has it alone.
type mapping struct {
	file  *os.File
	bytes []byte
}

// drop gives the mapping's pages back (dropPages).
func (mapped *mapping) drop() {
	if mapped != nil {
		dropPages(mapped.file, mapped.bytes)
	}
}

// close closes the mapping's file, after which its pages are given back no
// more. The mapping itself stays (mapFile).
func (mapped *mapping) close() {
	if mapped != nil {
		mapped.file.Close()
	}
}

// release gives the mapping's pages back and closes its file.
func (mapped *mapping) release() {
	mapped.drop()
	mapped.close()
}
