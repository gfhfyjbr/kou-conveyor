// Package highlight colours source files for the browser cockpit's file
// viewer. It tokenizes a text with chroma's lexers — the lexer found by the
// file's name, else by its content — and names each run of it by a class of
// token, which the page's style sheet colours: the text goes to the page
// once, as text, and the runs beside it as numbers, so nothing the file
// holds is ever HTML.
//
// Highlighting a large file takes a while — chroma reads 1 to 3 MB a
// second — so it runs against a deadline: what is done by then comes back
// at once, the rest follows (Cache).
package highlight

import (
	"path"
	"strings"
	"sync"
	"time"
	"unicode/utf8"

	"github.com/alecthomas/chroma/v2"
	"github.com/alecthomas/chroma/v2/lexers"
)

// Classes are the classes of tokens a page colours, by the index the runs
// give; 0 is plain text.
var Classes = []string{
	"", "k", "kc", "kd", "kt", "kn", "nb", "nf", "nc", "nn", "nt", "na", "nd", "nv", "no", "py", "nl",
	"s", "se", "si", "sr", "sd", "ss", "m", "l", "o", "ow", "p", "c", "cp", "cs",
	"gd", "gi", "gh", "gu", "ge", "gs", "gp", "go", "gr", "err",
}

var classIndex = func() map[string]int32 {
	index := make(map[string]int32, len(Classes))
	for i, name := range Classes {
		index[name] = int32(i)
	}
	return index
}()

// classOf is the class of a type of token.
func classOf(t chroma.TokenType) string {
	switch {
	case t == chroma.KeywordConstant:
		return "kc"
	case t == chroma.KeywordDeclaration:
		return "kd"
	case t == chroma.KeywordType:
		return "kt"
	case t == chroma.KeywordNamespace:
		return "kn"
	case t.InCategory(chroma.Keyword):
		return "k"
	case t.InSubCategory(chroma.NameBuiltin):
		return "nb"
	case t.InSubCategory(chroma.NameFunction):
		return "nf"
	case t == chroma.NameClass || t == chroma.NameException:
		return "nc"
	case t == chroma.NameNamespace:
		return "nn"
	case t == chroma.NameTag:
		return "nt"
	case t == chroma.NameAttribute:
		return "na"
	case t == chroma.NameDecorator:
		return "nd"
	case t.InSubCategory(chroma.NameVariable):
		return "nv"
	case t == chroma.NameConstant || t == chroma.NameEntity:
		return "no"
	case t == chroma.NameProperty:
		return "py"
	case t == chroma.NameLabel:
		return "nl"
	case t.InCategory(chroma.Name):
		return ""
	case t == chroma.LiteralStringEscape:
		return "se"
	case t == chroma.LiteralStringInterpol:
		return "si"
	case t == chroma.LiteralStringRegex:
		return "sr"
	case t == chroma.LiteralStringDoc:
		return "sd"
	case t == chroma.LiteralStringSymbol || t == chroma.LiteralStringAtom:
		return "ss"
	case t.InSubCategory(chroma.LiteralString):
		return "s"
	case t.InSubCategory(chroma.LiteralNumber):
		return "m"
	case t.InCategory(chroma.Literal):
		return "l"
	case t == chroma.OperatorWord:
		return "ow"
	case t.InCategory(chroma.Operator):
		return "o"
	case t.InCategory(chroma.Punctuation) || t == chroma.TextPunctuation:
		return "p"
	case t.InSubCategory(chroma.CommentPreproc):
		return "cp"
	case t == chroma.CommentHashbang || t == chroma.CommentSpecial:
		return "cs"
	case t.InCategory(chroma.Comment):
		return "c"
	case t == chroma.GenericDeleted:
		return "gd"
	case t == chroma.GenericInserted:
		return "gi"
	case t == chroma.GenericHeading:
		return "gh"
	case t == chroma.GenericSubheading:
		return "gu"
	case t == chroma.GenericEmph:
		return "ge"
	case t == chroma.GenericStrong:
		return "gs"
	case t == chroma.GenericPrompt:
		return "gp"
	case t == chroma.GenericOutput:
		return "go"
	case t == chroma.GenericError || t == chroma.GenericTraceback:
		return "gr"
	case t == chroma.Error:
		return "err"
	}
	return ""
}

// Result is a text highlighted.
type Result struct {
	// Language names the lexer; "" for plain text.
	Language string `json:"language"`
	// Runs are pairs of a length, in UTF-16 code units as a page counts
	// them, and a class, by its index in Classes: the text, from its
	// start, run by run. Text past the last run is plain.
	Runs []int32 `json:"runs"`
	// Complete is set once the whole text is highlighted; otherwise the
	// runs cover its start, as far as the time given allowed.
	Complete bool `json:"complete"`
}

// lexerCache keeps the lexer found for a file name's extension, or its
// name: matching one against every lexer takes milliseconds.
var lexerCache sync.Map // key → chroma.Lexer, or nil

// lexerFor finds the lexer for a file: by its name, else by its content.
func lexerFor(name, text string) chroma.Lexer {
	base := path.Base(name)
	key := strings.ToLower(path.Ext(base))
	if key == "" || key == base {
		key = "name:" + base
	}
	if cached, ok := lexerCache.Load(key); ok {
		if cached != nil {
			return cached.(chroma.Lexer)
		}
	} else {
		lexer := lexers.Match(base)
		if lexer == nil || plain(lexer) {
			lexerCache.Store(key, nil)
		} else {
			lexerCache.Store(key, lexer)
			return lexer
		}
	}
	// A script without an extension says what it is on its first line.
	sample := text[:min(len(text), 8<<10)]
	if lexer := lexers.Analyse(sample); lexer != nil && !plain(lexer) {
		return lexer
	}
	return nil
}

// plain reports a lexer that finds nothing to colour.
func plain(lexer chroma.Lexer) bool {
	return lexer == lexers.Fallback || lexer.Config().Name == "plaintext"
}

// Language names the lexer for a file, "" for plain text.
func Language(name, text string) string {
	if lexer := lexerFor(name, text); lexer != nil {
		return lexer.Config().Name
	}
	return ""
}

// Highlight highlights text, the content of the file name, until deadline
// (none if zero).
func Highlight(name, text string, deadline time.Time) Result {
	lexer := lexerFor(name, text)
	if lexer == nil {
		return Result{Complete: true}
	}
	result := Result{Language: lexer.Config().Name}
	it, err := chroma.Coalesce(lexer).Tokenise(nil, text)
	if err != nil {
		return Result{Complete: true}
	}
	total := utf16Len(text)
	var covered int32
	class, length := int32(-1), int32(0)
	flush := func() {
		if length > 0 {
			result.Runs = append(result.Runs, length, class)
			covered += length
		}
	}
	for n := 0; ; n++ {
		token := it()
		if token == chroma.EOF {
			result.Complete = true
			break
		}
		if n&1023 == 1023 && !deadline.IsZero() && time.Now().After(deadline) {
			break
		}
		c := classIndex[classOf(token.Type)]
		l := utf16Len(token.Value)
		if c == class {
			length += l
			continue
		}
		flush()
		class, length = c, l
	}
	flush()
	// A lexer may add a newline the text did not end with.
	if over := covered - total; over > 0 {
		for i := len(result.Runs) - 2; i >= 0 && over > 0; i -= 2 {
			cut := min(over, result.Runs[i])
			result.Runs[i] -= cut
			over -= cut
			if result.Runs[i] == 0 {
				result.Runs = result.Runs[:i]
			}
		}
	}
	return result
}

// utf16Len is the length of s in UTF-16 code units.
func utf16Len(s string) int32 {
	n := int32(0)
	for i := 0; i < len(s); {
		r, size := utf8.DecodeRuneInString(s[i:])
		i += size
		if r >= 0x10000 {
			n += 2
		} else {
			n++
		}
	}
	return n
}
