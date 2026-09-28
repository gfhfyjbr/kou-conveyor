package contextbuilder

import (
	"cmp"
	"fmt"
	"path"
	"regexp"
	"strings"
)

// Linked files. A user's message can link files and folders by references
// in its text ("$cmd/main.go", "$cmd/main.go:10-40"): the host reads what
// the model should see of each as the message is sent, and the message
// records it (LinkedFile). The model reads them after the message's text,
// each under its label, its lines numbered as cat -n numbers them, with the
// lines left out and a way to read them. What a file showed is a snapshot:
// the file itself is there to read in full, and as it is now. A compaction
// keeps the labels of an answered message's files, not what they showed.

const linkedFilesPreface = "The files and folders the message above links to by its $ references, as they were when it was sent. Each shows the lines its reference asked for, or the start of the file; read the file itself for the rest, or for what it holds now."

// linkedText is the text of a message with the files it links after it.
func linkedText(text string, files []LinkedFile) string {
	if len(files) == 0 {
		return text
	}
	var b strings.Builder
	b.WriteString(text)
	b.WriteString("\n\n<linked_files>\n")
	b.WriteString(linkedFilesPreface)
	for _, file := range files {
		b.WriteString("\n")
		writeLinkedFile(&b, file)
	}
	b.WriteString("\n</linked_files>")
	return b.String()
}

func writeLinkedFile(b *strings.Builder, file LinkedFile) {
	tag := "file"
	if file.Directory {
		tag = "folder"
	}
	fmt.Fprintf(b, "<%s path=%s", tag, attribute(file.Path))
	if file.Label != "" {
		fmt.Fprintf(b, " label=%s", attribute(file.Label))
	}
	b.WriteString(">\n")
	switch {
	case file.Error != "":
		fmt.Fprintf(b, "Not read: %s.\n", strings.TrimRight(file.Error, ". "))
	case file.Directory:
		writeFolder(b, file)
	case file.Binary:
		fmt.Fprintf(b, "A binary file of %s, not shown.", formatBytes(file.Size))
		if imagePath(file.Path) {
			b.WriteString(" ViewImage shows an image.")
		}
		b.WriteString("\n")
	case file.Lines == 0 && file.Size == 0:
		b.WriteString("The file is empty.\n")
	default:
		writeLines(b, file)
	}
	fmt.Fprintf(b, "</%s>", tag)
}

// writeLines writes the lines a file shows, numbered, and what it leaves
// out.
func writeLines(b *strings.Builder, file LinkedFile) {
	size := formatBytes(file.Size)
	switch {
	case file.From == 0 && file.Lines > 0:
		fmt.Fprintf(b, "No lines shown: the file has %s (%s).\n", count(file.Lines, "line", "lines"), size)
	case file.From == 0:
		fmt.Fprintf(b, "No lines shown: the file is %s.\n", size)
	case file.From == 1 && file.To == file.Lines:
		fmt.Fprintf(b, "All %s (%s):\n", count(file.Lines, "line", "lines"), size)
	case file.Lines > 0:
		fmt.Fprintf(b, "%s of %d (%s):\n", upper(lineRange(file.From, file.To)), file.Lines, size)
	default:
		fmt.Fprintf(b, "%s of a file of %s:\n", upper(lineRange(file.From, file.To)), size)
	}
	if file.From > 0 {
		for n, line := range contentLines(file.Content) {
			fmt.Fprintf(b, "%6d\t%s\n", file.From+n, line)
		}
	}
	// The lines left out, and a command that reads some of them: those
	// after the lines shown, else those before.
	var missing []string
	if file.From > 1 {
		missing = append(missing, lineRange(1, file.From-1))
	}
	switch {
	case file.To > 0 && file.Lines > file.To:
		missing = append(missing, lineRange(file.To+1, file.Lines))
	case file.To > 0 && file.Lines == 0:
		missing = append(missing, fmt.Sprintf("the lines after line %d", file.To))
	}
	from, to := file.To+1, file.To+readChunk
	if file.From == 0 {
		to = readChunk
	}
	switch {
	case file.From == 0:
		// None shown: the file is there to read.
		if file.Lines > 0 {
			to = min(file.Lines, readChunk)
		}
		fmt.Fprintf(b, "Read it from the file when you need it, e.g. `sed -n '1,%dp' %s`.\n", to, shellQuote(file.Path))
		return
	case len(missing) == 0:
		return
	case file.Lines > file.To:
		to = min(to, file.Lines)
	case file.Lines > 0:
		from, to = max(1, file.From-readChunk), file.From-1
	}
	fmt.Fprintf(b, "Not shown: %s. Read them from the file when you need them, e.g. `sed -n '%d,%dp' %s`.\n",
		strings.Join(missing, " and "), from, to, shellQuote(file.Path))
}

// readChunk is how many lines the command that reads what a file left out
// reads at most.
const readChunk = 1000

func writeFolder(b *strings.Builder, file LinkedFile) {
	switch {
	case file.Lines == 0:
		b.WriteString("The folder is empty.\n")
		return
	case file.From == 1 && file.To == file.Lines:
		fmt.Fprintf(b, "%s:\n", upper(count(file.Lines, "entry", "entries")))
	default:
		fmt.Fprintf(b, "Entries %d-%d of %d:\n", file.From, file.To, file.Lines)
	}
	for _, entry := range contentLines(file.Content) {
		b.WriteString(entry + "\n")
	}
	if file.Lines > file.To {
		fmt.Fprintf(b, "Not shown: %d more; list them with `ls -A %s`.\n", file.Lines-file.To, shellQuote(file.Path))
	}
}

// answeredText is what a compaction keeps of a message the model answered:
// its text, and where images or linked files came with it, their labels.
// The images, and what the files showed, are left out.
func answeredText(message Message) string {
	text := message.Text
	if len(message.Images) != 0 {
		labels := make([]string, 0, len(message.Images))
		for index, image := range message.Images {
			labels = append(labels, cmp.Or(image.Label, fmt.Sprintf("image %d", index+1)))
		}
		text += "\n[The user attached " + strings.Join(labels, ", ") + " here; images are left out after a compaction.]"
	}
	if len(message.Files) != 0 {
		labels := make([]string, 0, len(message.Files))
		for _, file := range message.Files {
			labels = append(labels, cmp.Or(file.Label, "$"+file.Path))
		}
		text += "\n[The user linked " + strings.Join(labels, ", ") + " here; what they showed is left out after a compaction: read the files themselves.]"
	}
	return text
}

// contentLines splits what a file showed into its lines.
func contentLines(content string) []string {
	if content == "" {
		return nil
	}
	return strings.Split(strings.TrimSuffix(content, "\n"), "\n")
}

func lineRange(from, to int) string {
	if from == to {
		return fmt.Sprintf("line %d", from)
	}
	return fmt.Sprintf("lines %d-%d", from, to)
}

func count(n int, one, many string) string {
	if n == 1 {
		return "1 " + one
	}
	return fmt.Sprintf("%d %s", n, many)
}

func upper(text string) string {
	if text == "" {
		return text
	}
	return strings.ToUpper(text[:1]) + text[1:]
}

func formatBytes(n int64) string {
	switch {
	case n < 1<<10:
		return count(int(n), "byte", "bytes")
	case n < 1<<20:
		return fmt.Sprintf("%.1f KB", float64(n)/(1<<10))
	case n < 1<<30:
		return fmt.Sprintf("%.1f MB", float64(n)/(1<<20))
	}
	return fmt.Sprintf("%.1f GB", float64(n)/(1<<30))
}

// attribute quotes a value for a tag's attribute.
func attribute(value string) string {
	value = strings.Join(strings.Fields(strings.NewReplacer("&", "&amp;", `"`, "&quot;", "<", "&lt;", ">", "&gt;").Replace(value)), " ")
	return `"` + value + `"`
}

var shellSafe = regexp.MustCompile(`^[A-Za-z0-9_./@%+=:,~-]+$`)

// shellQuote quotes a path for a command line, where it needs quoting.
func shellQuote(value string) string {
	if shellSafe.MatchString(value) && !strings.HasPrefix(value, "-") && !strings.HasPrefix(value, "~") {
		return value
	}
	return "'" + strings.ReplaceAll(value, "'", `'\''`) + "'"
}

func imagePath(name string) bool {
	switch strings.ToLower(path.Ext(name)) {
	case ".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp", ".tif", ".tiff":
		return true
	}
	return false
}
