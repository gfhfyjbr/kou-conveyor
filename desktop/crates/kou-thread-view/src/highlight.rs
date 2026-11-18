//! Colours for fenced code, by a scanner per family of languages rather than
//! a grammar: keywords, strings, comments, numbers, types and calls, which is
//! most of what makes code read faster, for no dependency and a pass over the
//! text. A language it does not know is left as it is.

use std::ops::Range;

/// What a piece of code is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Token {
    Keyword,
    String,
    Comment,
    Number,
    /// A type, or a constant such as `true`.
    Type,
    /// A function or macro being called.
    Call,
    /// An added line of a diff.
    Added,
    /// A removed line of a diff.
    Removed,
    /// A hunk header or a file name of a diff.
    Meta,
}

/// How a family of languages writes comments and strings.
struct Syntax {
    line_comments: &'static [&'static str],
    block_comment: Option<(&'static str, &'static str)>,
    /// Quotes that open a string; the backtick spans lines.
    quotes: &'static [char],
    /// `'a` is a lifetime, not a string.
    lifetimes: bool,
    /// `"""` opens a string that spans lines.
    triple: bool,
    keywords: &'static [&'static str],
    constants: &'static [&'static str],
    /// Keywords are matched without regard to case.
    any_case: bool,
}

const RUST: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern",
    "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref",
    "return", "self", "Self", "static", "struct", "super", "trait", "type", "unsafe", "use",
    "where", "while",
];
const GO: &[&str] = &[
    "break",
    "case",
    "chan",
    "const",
    "continue",
    "default",
    "defer",
    "else",
    "fallthrough",
    "for",
    "func",
    "go",
    "goto",
    "if",
    "import",
    "interface",
    "map",
    "package",
    "range",
    "return",
    "select",
    "struct",
    "switch",
    "type",
    "var",
];
const SCRIPT: &[&str] = &[
    "as",
    "async",
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "default",
    "delete",
    "do",
    "else",
    "export",
    "extends",
    "finally",
    "for",
    "from",
    "function",
    "if",
    "import",
    "in",
    "instanceof",
    "interface",
    "let",
    "new",
    "of",
    "return",
    "static",
    "super",
    "switch",
    "this",
    "throw",
    "try",
    "type",
    "typeof",
    "var",
    "void",
    "while",
    "yield",
];
const C: &[&str] = &[
    "auto",
    "break",
    "case",
    "class",
    "const",
    "continue",
    "default",
    "delete",
    "do",
    "else",
    "enum",
    "extern",
    "for",
    "goto",
    "if",
    "inline",
    "namespace",
    "new",
    "private",
    "protected",
    "public",
    "return",
    "sizeof",
    "static",
    "struct",
    "switch",
    "template",
    "this",
    "typedef",
    "union",
    "using",
    "virtual",
    "void",
    "volatile",
    "while",
    "func",
    "var",
    "let",
    "import",
    "package",
    "fun",
    "val",
    "override",
    "interface",
    "extends",
    "implements",
];
const PYTHON: &[&str] = &[
    "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del", "elif",
    "else", "except", "finally", "for", "from", "global", "if", "import", "in", "is", "lambda",
    "nonlocal", "not", "or", "pass", "raise", "return", "try", "while", "with", "yield",
];
const SHELL: &[&str] = &[
    "case", "do", "done", "elif", "else", "esac", "export", "fi", "for", "function", "if", "in",
    "local", "return", "set", "then", "until", "while",
];
const SQL: &[&str] = &[
    "select",
    "from",
    "where",
    "and",
    "or",
    "not",
    "insert",
    "into",
    "values",
    "update",
    "set",
    "delete",
    "create",
    "table",
    "index",
    "drop",
    "alter",
    "join",
    "left",
    "right",
    "inner",
    "outer",
    "on",
    "group",
    "by",
    "order",
    "having",
    "limit",
    "as",
    "is",
    "null",
    "primary",
    "key",
    "references",
    "begin",
    "commit",
    "with",
];
const CONSTANTS: &[&str] = &[
    "true",
    "false",
    "null",
    "nil",
    "None",
    "True",
    "False",
    "undefined",
    "NULL",
    "iota",
];

/// The syntax of a fence's language, if it is one this knows.
fn syntax(lang: &str) -> Option<Syntax> {
    let c_like = |keywords| Syntax {
        line_comments: &["//"],
        block_comment: Some(("/*", "*/")),
        quotes: &['"', '\''],
        lifetimes: false,
        triple: false,
        keywords,
        constants: CONSTANTS,
        any_case: false,
    };
    let hash = |keywords, triple| Syntax {
        line_comments: &["#"],
        block_comment: None,
        quotes: &['"', '\''],
        lifetimes: false,
        triple,
        keywords,
        constants: CONSTANTS,
        any_case: false,
    };
    Some(match lang.to_ascii_lowercase().as_str() {
        "rust" | "rs" => Syntax {
            lifetimes: true,
            ..c_like(RUST)
        },
        "go" | "golang" => Syntax {
            quotes: &['"', '\'', '`'],
            ..c_like(GO)
        },
        "js" | "javascript" | "jsx" | "ts" | "typescript" | "tsx" | "mjs" | "cjs" => Syntax {
            quotes: &['"', '\'', '`'],
            ..c_like(SCRIPT)
        },
        "c" | "h" | "cpp" | "c++" | "cc" | "hpp" | "cs" | "csharp" | "java" | "kotlin" | "kt"
        | "swift" | "scala" | "dart" | "zig" | "objc" => c_like(C),
        "python" | "py" => hash(PYTHON, true),
        "sh" | "bash" | "zsh" | "shell" | "console" | "fish" => hash(SHELL, false),
        "toml" | "yaml" | "yml" | "ini" | "conf" | "dockerfile" | "make" | "makefile" => {
            hash(&[], false)
        }
        "ruby" | "rb" | "perl" | "r" => hash(PYTHON, false),
        "json" | "jsonc" | "json5" => Syntax {
            quotes: &['"'],
            ..c_like(&[])
        },
        "sql" => Syntax {
            line_comments: &["--"],
            block_comment: Some(("/*", "*/")),
            quotes: &['\''],
            lifetimes: false,
            triple: false,
            keywords: SQL,
            constants: CONSTANTS,
            any_case: true,
        },
        _ => return None,
    })
}

/// The coloured pieces of a code block, in order and not overlapping.
pub(crate) fn highlight(lang: Option<&str>, text: &str) -> Vec<(Range<usize>, Token)> {
    let Some(lang) = lang.map(str::trim).filter(|lang| !lang.is_empty()) else {
        return Vec::new();
    };
    let lang = lang.split([',', '{', ' ']).next().unwrap_or(lang);
    if matches!(lang.to_ascii_lowercase().as_str(), "diff" | "patch") {
        return diff(text);
    }
    match syntax(lang) {
        Some(syntax) => scan(&syntax, text),
        None => Vec::new(),
    }
}

/// A diff: added and removed lines, and its headers.
fn diff(text: &str) -> Vec<(Range<usize>, Token)> {
    let mut found = Vec::new();
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        let body = line.trim_end_matches('\n');
        let token = if body.starts_with("+++") || body.starts_with("---") || body.starts_with("@@")
        {
            Some(Token::Meta)
        } else if body.starts_with('+') {
            Some(Token::Added)
        } else if body.starts_with('-') {
            Some(Token::Removed)
        } else {
            None
        };
        if let Some(token) = token
            && !body.is_empty()
        {
            found.push((at..at + body.len(), token));
        }
        at += line.len();
    }
    found
}

fn scan(syntax: &Syntax, text: &str) -> Vec<(Range<usize>, Token)> {
    let mut found = Vec::new();
    let mut at = 0;
    let starts = |at: usize, pattern: &str| text[at..].starts_with(pattern);
    while at < text.len() {
        let ch = text[at..].chars().next().unwrap_or(' ');
        // Comments.
        if let Some(open) = syntax.line_comments.iter().find(|open| starts(at, open)) {
            // `#` in a shell word, such as `$#` or `a#b`, is no comment.
            let word = at > 0 && *open == "#" && !text[..at].ends_with(char::is_whitespace);
            if !word {
                let end = text[at..].find('\n').map_or(text.len(), |end| at + end);
                found.push((at..end, Token::Comment));
                at = end;
                continue;
            }
        }
        if let Some((open, close)) = syntax.block_comment
            && starts(at, open)
        {
            let end = text[at + open.len()..]
                .find(close)
                .map_or(text.len(), |end| at + open.len() + end + close.len());
            found.push((at..end, Token::Comment));
            at = end;
            continue;
        }
        // Strings.
        if syntax.triple && (starts(at, "\"\"\"") || starts(at, "'''")) {
            let quote = &text[at..at + 3];
            let end = text[at + 3..]
                .find(quote)
                .map_or(text.len(), |end| at + 3 + end + 3);
            found.push((at..end, Token::String));
            at = end;
            continue;
        }
        if syntax.quotes.contains(&ch) {
            if ch == '\'' && syntax.lifetimes {
                // `'a'` is a character; `'a` a lifetime.
                let rest = &text[at + 1..];
                let ident = rest
                    .find(|c: char| !(c.is_alphanumeric() || c == '_'))
                    .unwrap_or(rest.len());
                if ident > 0 && !rest[ident..].starts_with('\'') {
                    found.push((at..at + 1 + ident, Token::Type));
                    at += 1 + ident;
                    continue;
                }
            }
            let end = string_end(text, at, ch);
            found.push((at..end, Token::String));
            at = end;
            continue;
        }
        // Numbers.
        if ch.is_ascii_digit() && !previous_is_word(text, at) {
            let end = at
                + text[at..]
                    .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '.'))
                    .unwrap_or(text.len() - at);
            found.push((at..end, Token::Number));
            at = end;
            continue;
        }
        // Words.
        if ch.is_alphabetic() || ch == '_' {
            let end = at
                + text[at..]
                    .find(|c: char| !(c.is_alphanumeric() || c == '_'))
                    .unwrap_or(text.len() - at);
            let word = &text[at..end];
            let keyword = if syntax.any_case {
                syntax
                    .keywords
                    .iter()
                    .any(|keyword| keyword.eq_ignore_ascii_case(word))
            } else {
                syntax.keywords.contains(&word)
            };
            let after = text[end..].trim_start_matches(' ');
            let token = if keyword {
                Some(Token::Keyword)
            } else if syntax.constants.contains(&word) {
                Some(Token::Type)
            } else if after.starts_with('(') || after.starts_with("!(") {
                Some(Token::Call)
            } else if word.chars().next().is_some_and(char::is_uppercase)
                && word.chars().any(char::is_lowercase)
            {
                Some(Token::Type)
            } else {
                None
            };
            if let Some(token) = token {
                // A macro's bang is part of its name.
                let end = if token == Token::Call
                    && after.starts_with('!')
                    && after.len() == text[end..].len()
                {
                    end + 1
                } else {
                    end
                };
                found.push((at..end, token));
                at = end;
            } else {
                at = end;
            }
            continue;
        }
        at += ch.len_utf8();
    }
    found
}

/// Where a string opened at `at` by `quote` ends: after its closing quote,
/// at the end of the line for an unclosed one (but a backtick's), or at the
/// end of the text.
fn string_end(text: &str, at: usize, quote: char) -> usize {
    let mut escaped = false;
    for (offset, ch) in text[at + 1..].char_indices() {
        let index = at + 1 + offset;
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' if quote != '`' => escaped = true,
            '\n' if quote != '`' => return index,
            ch if ch == quote => return index + ch.len_utf8(),
            _ => {}
        }
    }
    text.len()
}

fn previous_is_word(text: &str, at: usize) -> bool {
    text[..at]
        .chars()
        .next_back()
        .is_some_and(|ch| ch.is_alphanumeric() || ch == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pieces(lang: &str, text: &str) -> Vec<(String, Token)> {
        highlight(Some(lang), text)
            .into_iter()
            .map(|(range, token)| (text[range].to_owned(), token))
            .collect()
    }

    #[test]
    fn rust_reads_keywords_strings_comments_and_lifetimes() {
        let found = pieces(
            "rust",
            "fn main<'a>() { let x = \"hi \\\"you\\\"\"; // done\n println!(\"{}\", 42); Some(x) }",
        );
        assert_eq!(
            found,
            [
                ("fn".into(), Token::Keyword),
                ("'a".into(), Token::Type),
                ("let".into(), Token::Keyword),
                ("\"hi \\\"you\\\"\"".into(), Token::String),
                ("// done".into(), Token::Comment),
                ("println!".into(), Token::Call),
                ("\"{}\"".into(), Token::String),
                ("42".into(), Token::Number),
                ("Some".into(), Token::Call),
            ]
        );
        assert_eq!(pieces("rs", "'x'"), [("'x'".into(), Token::String)]);
    }

    #[test]
    fn other_families_and_unknown_languages() {
        assert_eq!(
            pieces("go", "func f() { s := `raw\nline` /* c */ }"),
            [
                ("func".into(), Token::Keyword),
                ("f".into(), Token::Call),
                ("`raw\nline`".into(), Token::String),
                ("/* c */".into(), Token::Comment),
            ]
        );
        assert_eq!(
            pieces(
                "python",
                "def f():\n    \"\"\"doc\"\"\"  # note\n    return None"
            ),
            [
                ("def".into(), Token::Keyword),
                ("f".into(), Token::Call),
                ("\"\"\"doc\"\"\"".into(), Token::String),
                ("# note".into(), Token::Comment),
                ("return".into(), Token::Keyword),
                ("None".into(), Token::Type),
            ]
        );
        // `$#` is a word of the shell, not a comment.
        assert_eq!(
            pieces("sh", "echo $# done # comment"),
            [
                ("done".into(), Token::Keyword),
                ("# comment".into(), Token::Comment)
            ]
        );
        assert_eq!(
            pieces("SQL", "SELECT name FROM t -- all"),
            [
                ("SELECT".into(), Token::Keyword),
                ("FROM".into(), Token::Keyword),
                ("-- all".into(), Token::Comment),
            ]
        );
        assert_eq!(
            pieces("diff", "@@ -1 +1 @@\n-old\n+new\n same"),
            [
                ("@@ -1 +1 @@".into(), Token::Meta),
                ("-old".into(), Token::Removed),
                ("+new".into(), Token::Added),
            ]
        );
        assert!(highlight(Some("brainfuck"), "+++").is_empty());
        assert!(highlight(None, "fn x() {}").is_empty());
        // Unclosed strings stop at the end of their line.
        assert_eq!(
            pieces("js", "let s = \"open\nlet t"),
            [
                ("let".into(), Token::Keyword),
                ("\"open".into(), Token::String),
                ("let".into(), Token::Keyword),
            ]
        );
    }
}
