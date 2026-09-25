package plugin

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"hash"
	"io"
	"io/fs"
	"os"
	"path"
	"path/filepath"
	"slices"
	"strings"
	"time"
)

// Plugins change while they run: a user edits a script, a workspace brings
// a new tool, a built-in plugin is worked on in a checkout. Nothing needs a
// restart for that: whatever reads plugins fingerprints where they come
// from, and reads them again once a fingerprint changes. Fingerprints go by
// the files' names, sizes, modes and times, so taking one costs a stat per
// file; the few directories plugins live in are cheap to look at every
// second.

// maxWatchedFiles bounds the files a fingerprint looks at under one root, so
// a plugin that vendors a large tree does not make watching expensive.
const maxWatchedFiles = 4000

// Fingerprint summarizes the files under paths, directories or files, and
// changes when one of them is added, removed or written. A directory's
// entries that are links to directories are followed one level deep, as a
// plugin being worked on is often linked into the plugins directory. Hidden
// files and node_modules are left out.
func Fingerprint(paths ...string) string {
	digest := sha256.New()
	for _, root := range paths {
		fmt.Fprintf(digest, "root %s\n", root)
		fingerprintTree(digest, root, 1)
	}
	return hex.EncodeToString(digest.Sum(nil))[:20]
}

func fingerprintTree(digest hash.Hash, root string, follow int) {
	info, err := os.Stat(root)
	if err != nil {
		fmt.Fprintf(digest, "missing\n")
		return
	}
	if !info.IsDir() {
		fmt.Fprintf(digest, "file %d %d %d\n", info.Size(), info.ModTime().UnixNano(), info.Mode())
		return
	}
	resolved, err := filepath.EvalSymlinks(root)
	if err != nil {
		resolved = root
	}
	count := 0
	var links []string
	_ = filepath.WalkDir(resolved, func(current string, entry fs.DirEntry, err error) error {
		if err != nil {
			return nil
		}
		name := entry.Name()
		if current != resolved && (strings.HasPrefix(name, ".") || name == "node_modules") {
			if entry.IsDir() {
				return filepath.SkipDir
			}
			return nil
		}
		if count++; count > maxWatchedFiles {
			return filepath.SkipAll
		}
		relative, _ := filepath.Rel(resolved, current)
		if entry.Type()&fs.ModeSymlink != 0 && follow > 0 {
			if target, err := os.Stat(current); err == nil && target.IsDir() {
				links = append(links, relative)
				return nil
			}
		}
		if entry.IsDir() {
			// A directory counts by its name: its time changes with any
			// file in it, hidden ones too, and its files are counted anyway.
			fmt.Fprintf(digest, "%s/\n", filepath.ToSlash(relative))
			return nil
		}
		info, err := entry.Info()
		if err != nil {
			return nil
		}
		fmt.Fprintf(digest, "%s %d %d %d\n", filepath.ToSlash(relative), info.Size(), info.ModTime().UnixNano(), info.Mode())
		return nil
	})
	for _, link := range links {
		fmt.Fprintf(digest, "link %s\n", filepath.ToSlash(link))
		fingerprintTree(digest, filepath.Join(resolved, link), follow-1)
	}
}

// Versions fingerprints a plugin's files: style covers its style sheets,
// code every other file, the manifest among them. A browser that loaded the
// plugin takes up a new code version by loading the plugin again, and a new
// style version by swapping its style sheets.
func (current Plugin) Versions() (code, style string) {
	codeDigest, styleDigest := sha256.New(), sha256.New()
	fmt.Fprintf(codeDigest, "%s %s\n", current.Name, current.Source)
	switch {
	case current.Directory != "":
		resolved, err := filepath.EvalSymlinks(current.Directory)
		if err != nil {
			resolved = current.Directory
		}
		count := 0
		_ = filepath.WalkDir(resolved, func(file string, entry fs.DirEntry, err error) error {
			if err != nil || entry.IsDir() {
				if err == nil && file != resolved && (strings.HasPrefix(entry.Name(), ".") || entry.Name() == "node_modules") {
					return filepath.SkipDir
				}
				return nil
			}
			if count++; count > maxWatchedFiles {
				return filepath.SkipAll
			}
			info, err := os.Stat(file) // links are followed
			if err != nil {
				return nil
			}
			relative, _ := filepath.Rel(resolved, file)
			digest := codeDigest
			if strings.EqualFold(filepath.Ext(file), ".css") {
				digest = styleDigest
			}
			fmt.Fprintf(digest, "%s %d %d\n", filepath.ToSlash(relative), info.Size(), info.ModTime().UnixNano())
			return nil
		})
	case current.Files != nil:
		// Files compiled in have no times: their content counts.
		_ = fs.WalkDir(current.Files, ".", func(file string, entry fs.DirEntry, err error) error {
			if err != nil || entry.IsDir() {
				return nil
			}
			digest := codeDigest
			if strings.EqualFold(path.Ext(file), ".css") {
				digest = styleDigest
			}
			fmt.Fprintf(digest, "%s\n", file)
			if opened, err := current.Files.Open(file); err == nil {
				_, _ = io.Copy(digest, opened)
				opened.Close()
			}
			return nil
		})
	default:
		encoded := fmt.Sprintf("%v", current.Manifest)
		codeDigest.Write([]byte(encoded))
	}
	return hex.EncodeToString(codeDigest.Sum(nil))[:16], hex.EncodeToString(styleDigest.Sum(nil))[:16]
}

// Sources are the files and directories Discover reads for options: the
// user's plugins and plugins.json, the workspace's plugins, and the
// directories of built-in plugins that live on disk.
func Sources(options Options) []string {
	var sources []string
	for _, builtin := range options.Builtins {
		if builtin.Directory != "" {
			sources = append(sources, builtin.Directory)
		}
	}
	if options.ConfigDirectory != "" {
		sources = append(sources, UserDirectory(options.ConfigDirectory), SettingsPath(options.ConfigDirectory))
	}
	if options.Workspace != "" {
		sources = append(sources, WorkspaceDirectory(options.Workspace))
	}
	return slices.Compact(sources)
}

// Watch calls changed, from its own goroutine, whenever the fingerprint of
// sources changes, looking every interval until ctx ends. sources is asked
// again every time, so what it watches may change.
func Watch(ctx context.Context, interval time.Duration, sources func() []string, changed func()) {
	last := Fingerprint(sources()...)
	ticker := time.NewTicker(interval)
	defer ticker.Stop()
	for {
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
		}
		if now := Fingerprint(sources()...); now != last {
			last = now
			changed()
		}
	}
}
