package main

import (
	"bytes"
	"context"
	"errors"
	"net/url"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"slices"
	"strings"
	"time"
)

// Images reach the composer three ways: ctrl+v reads one from the system
// clipboard; a paste that is only paths of image files, as dragging files
// onto the terminal or pasting files copied in a file manager gives, brings
// those files; and a paste with nothing in it, which some terminals send for
// ⌘V when the clipboard holds an image and no text, reads the clipboard.
// Terminals keep ⌘V for their own paste, which only knows text, so ctrl+v is
// the key that always works.

// errNoClipboardImage reports a clipboard that holds no image.
var errNoClipboardImage = errors.New("the clipboard holds no image")

// readClipboardImage returns the image on the system clipboard; tests
// replace it.
var readClipboardImage = systemClipboardImage

// imageExtensions are the files a paste of paths attaches.
var imageExtensions = []string{".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp", ".tif", ".tiff"}

func systemClipboardImage(ctx context.Context) ([]byte, error) {
	ctx, cancel := context.WithTimeout(ctx, 10*time.Second)
	defer cancel()
	switch runtime.GOOS {
	case "darwin":
		return macClipboardImage(ctx)
	case "windows":
		return nil, errNoClipboardImage
	}
	return unixClipboardImage(ctx)
}

// macClipboardImage asks AppleScript for the clipboard as an image: PNG,
// which screenshots put there, TIFF or JPEG, or an image file Finder copied.
func macClipboardImage(ctx context.Context) ([]byte, error) {
	info, err := exec.CommandContext(ctx, "osascript", "-e", "clipboard info").Output()
	if err != nil {
		return nil, errNoClipboardImage
	}
	for _, class := range []string{"PNGf", "TIFF", "JPEG", "GIFf"} {
		if !bytes.Contains(info, []byte("«class "+class+"»")) {
			continue
		}
		file, err := os.CreateTemp("", "kou-conveyor-clipboard-*")
		if err != nil {
			return nil, err
		}
		path := file.Name()
		file.Close()
		defer os.Remove(path)
		script := []string{
			"on run argv",
			"set f to open for access POSIX file (item 1 of argv) with write permission",
			"try",
			"write (the clipboard as «class " + class + "») to f",
			"end try",
			"close access f",
			"end run",
		}
		args := []string{}
		for _, line := range script {
			args = append(args, "-e", line)
		}
		if exec.CommandContext(ctx, "osascript", append(args, path)...).Run() != nil {
			continue
		}
		if data, err := os.ReadFile(path); err == nil && len(data) != 0 {
			return data, nil
		}
	}
	if bytes.Contains(info, []byte("«class furl»")) {
		out, err := exec.CommandContext(ctx, "osascript", "-e", "POSIX path of (the clipboard as «class furl»)").Output()
		if err == nil {
			if path := strings.TrimSpace(string(out)); isImagePath(path) {
				return os.ReadFile(path)
			}
		}
	}
	return nil, errNoClipboardImage
}

// unixClipboardImage reads an image from the Wayland or X clipboard, with
// wl-paste or xclip, whichever the session has.
func unixClipboardImage(ctx context.Context) ([]byte, error) {
	type tool struct {
		list func() []string
		read func(string) []string
	}
	var tools []tool
	if os.Getenv("WAYLAND_DISPLAY") != "" {
		tools = append(tools, tool{
			list: func() []string { return []string{"wl-paste", "--list-types"} },
			read: func(kind string) []string { return []string{"wl-paste", "--no-newline", "--type", kind} },
		})
	}
	if os.Getenv("DISPLAY") != "" {
		tools = append(tools, tool{
			list: func() []string { return []string{"xclip", "-selection", "clipboard", "-t", "TARGETS", "-o"} },
			read: func(kind string) []string { return []string{"xclip", "-selection", "clipboard", "-t", kind, "-o"} },
		})
	}
	for _, t := range tools {
		list := t.list()
		out, err := exec.CommandContext(ctx, list[0], list[1:]...).Output()
		if err != nil {
			continue
		}
		types := strings.Fields(string(out))
		for _, kind := range []string{"image/png", "image/jpeg", "image/gif", "image/webp", "image/bmp", "image/tiff"} {
			if !slices.Contains(types, kind) {
				continue
			}
			read := t.read(kind)
			if data, err := exec.CommandContext(ctx, read[0], read[1:]...).Output(); err == nil && len(data) != 0 {
				return data, nil
			}
		}
		// Files copied in a file manager.
		if slices.Contains(types, "text/uri-list") {
			read := t.read("text/uri-list")
			if out, err := exec.CommandContext(ctx, read[0], read[1:]...).Output(); err == nil {
				if paths := imagePaths(string(out)); len(paths) != 0 {
					return os.ReadFile(paths[0])
				}
			}
		}
	}
	return nil, errNoClipboardImage
}

// isImagePath reports an existing file that is an image by its name.
func isImagePath(path string) bool {
	if !slices.Contains(imageExtensions, strings.ToLower(filepath.Ext(path))) {
		return false
	}
	info, err := os.Stat(path)
	return err == nil && info.Mode().IsRegular()
}

// imagePaths reads pasted text as paths of image files: one or more, as
// shells quote them (dragging files onto a terminal escapes their spaces),
// or file:// URLs. Text that is anything else has none.
func imagePaths(text string) []string {
	text = strings.TrimSpace(text)
	if text == "" || len(text) > 64<<10 {
		return nil
	}
	var paths []string
	for _, word := range shellWords(text) {
		if rest, ok := strings.CutPrefix(word, "file://"); ok {
			if parsed, err := url.Parse("file://" + rest); err == nil {
				word = parsed.Path
			}
		}
		if rest, ok := strings.CutPrefix(word, "~/"); ok {
			if home, err := os.UserHomeDir(); err == nil {
				word = filepath.Join(home, rest)
			}
		}
		if !filepath.IsAbs(word) || !isImagePath(word) {
			return nil
		}
		paths = append(paths, word)
	}
	return paths
}

// shellWords splits text into words as a shell would: at unquoted blanks,
// with quotes and backslash escapes taken out.
func shellWords(text string) []string {
	var words []string
	var word strings.Builder
	started := false
	quote := rune(0)
	escaped := false
	for _, r := range text {
		switch {
		case escaped:
			word.WriteRune(r)
			escaped = false
		case r == '\\' && quote != '\'':
			escaped, started = true, true
		case quote != 0 && r == quote:
			quote = 0
		case quote != 0:
			word.WriteRune(r)
		case r == '\'' || r == '"':
			quote, started = r, true
		case r == ' ' || r == '\t' || r == '\n' || r == '\r':
			if started {
				words = append(words, word.String())
				word.Reset()
				started = false
			}
		default:
			word.WriteRune(r)
			started = true
		}
	}
	if started {
		words = append(words, word.String())
	}
	return words
}
