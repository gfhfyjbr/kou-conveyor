package skill

import (
	"bytes"
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"sync"

	"github.com/gfhfyjbr/kou-conveyor/harness/plugin"
)

// The agent reads a skill from disk: SkillUse loads its SKILL.md, and the
// agent reads the files beside it that it names. So the skills of a plugin
// compiled into the program — the built-in guide's — are written out, to
// builtin/<plugin>-<hash> in kou-conveyor's configuration directory, named
// after their content: programs of other versions keep theirs apart, and
// the runner and the cockpits of one version share one copy. A process
// checks the files once, writing those missing or changed; afterwards it
// only looks that the directory is still there.

// checked holds the directories whose files this process checked.
var checked sync.Map

// pluginSkills is the directory of a plugin's skills: in the plugin's
// directory, or, for a plugin compiled into the program, where written puts
// them — nowhere, without a configuration directory.
func pluginSkills(current plugin.Plugin, configDirectory string) (string, error) {
	if current.Directory != "" || current.Files == nil {
		return current.Resolve(current.Skills)
	}
	if configDirectory == "" {
		return "", nil
	}
	files, err := current.Sub(current.Skills)
	if err != nil {
		return "", err
	}
	return written(files, filepath.Join(configDirectory, "builtin"), current.Name)
}

// written writes files, a plugin's skills, to a directory of root named
// after the plugin and their content, and returns it.
func written(files fs.FS, root, name string) (string, error) {
	var paths []string
	contents := map[string][]byte{}
	digest := sha256.New()
	err := fs.WalkDir(files, ".", func(path string, entry fs.DirEntry, err error) error {
		if err != nil || entry.IsDir() {
			return err
		}
		content, err := fs.ReadFile(files, path)
		if err != nil {
			return err
		}
		paths, contents[path] = append(paths, path), content
		fmt.Fprintf(digest, "%s %d\n", path, len(content))
		digest.Write(content)
		return nil
	})
	if err != nil {
		return "", err
	}
	directory := filepath.Join(root, name+"-"+hex.EncodeToString(digest.Sum(nil))[:12])
	if _, done := checked.Load(directory); done {
		if info, err := os.Stat(directory); err == nil && info.IsDir() {
			return directory, nil
		}
	}
	for _, path := range paths {
		target := filepath.Join(directory, filepath.FromSlash(path))
		if existing, err := os.ReadFile(target); err == nil && bytes.Equal(existing, contents[path]) {
			continue
		}
		if err := writeFile(target, contents[path]); err != nil {
			return "", fmt.Errorf("write %s: %w", target, err)
		}
	}
	checked.Store(directory, struct{}{})
	return directory, nil
}

// writeFile replaces a file whole: a reader sees the old content or the
// new, never part of it.
func writeFile(path string, content []byte) error {
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return err
	}
	temporary, err := os.CreateTemp(filepath.Dir(path), "."+filepath.Base(path)+".*.tmp")
	if err != nil {
		return err
	}
	_, err = temporary.Write(content)
	if closeErr := temporary.Close(); err == nil {
		err = closeErr
	}
	if err == nil {
		err = os.Chmod(temporary.Name(), 0o644)
	}
	if err == nil {
		err = os.Rename(temporary.Name(), path)
	}
	if err != nil {
		os.Remove(temporary.Name())
	}
	return err
}
