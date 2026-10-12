package worktree

import (
	"io"
	"io/fs"
	"os"
	"path/filepath"
)

// copyInto copies the file or tree at source to target, which does not
// exist yet: through a sibling it renames at the end, so that a copy cut
// short is not taken for one made. The tree, or else each file, is cloned
// where the file system can (clone), else copied; links are copied as
// links; what is neither a file, a folder nor a link is left out.
func copyInto(source, target string) error {
	partial := target + ".kou-partial"
	// A copy an earlier attempt left unfinished.
	if err := os.RemoveAll(partial); err != nil {
		return err
	}
	// A whole tree at once, where the file system clones folders.
	if clone(source, partial) == nil {
		return os.Rename(partial, target)
	}
	os.RemoveAll(partial)
	err := filepath.WalkDir(source, func(path string, entry fs.DirEntry, err error) error {
		if err != nil {
			return err
		}
		rel, err := filepath.Rel(source, path)
		if err != nil {
			return err
		}
		to := filepath.Join(partial, rel)
		info, err := entry.Info()
		if err != nil {
			return err
		}
		switch mode := info.Mode(); {
		case mode.IsDir():
			return os.MkdirAll(to, mode.Perm()|0o700)
		case mode&fs.ModeSymlink != 0:
			link, err := os.Readlink(path)
			if err != nil {
				return err
			}
			return os.Symlink(link, to)
		case mode.IsRegular():
			return copyFile(path, to, mode.Perm())
		}
		return nil
	})
	if err != nil {
		os.RemoveAll(partial)
		return err
	}
	return os.Rename(partial, target)
}

func copyFile(source, target string, mode fs.FileMode) error {
	if clone(source, target) == nil {
		return nil
	}
	in, err := os.Open(source)
	if err != nil {
		return err
	}
	defer in.Close()
	out, err := os.OpenFile(target, os.O_WRONLY|os.O_CREATE|os.O_EXCL, mode)
	if err != nil {
		return err
	}
	if _, err := io.Copy(out, in); err != nil {
		out.Close()
		return err
	}
	return out.Close()
}
