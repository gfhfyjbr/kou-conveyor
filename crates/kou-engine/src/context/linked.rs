use super::{ExternalMessage, LinkedFile};
use std::fmt::Write;
const PREFACE: &str = "The files and folders the message above links to by its $ references, as they were when it was sent. Each shows the lines its reference asked for, or the start of the file; read the file itself for the rest, or for what it holds now.";

pub(super) fn linked_text(text: &str, files: &[LinkedFile]) -> String {
    if files.is_empty() {
        return text.into();
    }
    let mut out = format!("{text}\n\n<linked_files>\n{PREFACE}");
    for file in files {
        out.push('\n');
        write_file(&mut out, file);
    }
    out.push_str("\n</linked_files>");
    out
}
fn write_file(out: &mut String, file: &LinkedFile) {
    let tag = if file.directory { "folder" } else { "file" };
    write!(out, "<{tag} path={}", attribute(&file.path)).unwrap();
    if !file.label.is_empty() {
        write!(out, " label={}", attribute(&file.label)).unwrap();
    }
    out.push_str(">\n");
    if !file.error.is_empty() {
        writeln!(
            out,
            "Not read: {}.",
            file.error.trim_end_matches(['.', ' '])
        )
        .unwrap();
    } else if file.directory {
        write_folder(out, file);
    } else if file.binary {
        write!(
            out,
            "A binary file of {}, not shown.",
            format_bytes(file.size)
        )
        .unwrap();
        let ext = file
            .path
            .rsplit_once('.')
            .map(|(_, ext)| ext.to_ascii_lowercase())
            .unwrap_or_default();
        if ["png", "jpg", "jpeg", "gif", "webp", "bmp", "tif", "tiff"].contains(&ext.as_str()) {
            out.push_str(" ViewImage shows an image.");
        }
        out.push('\n');
    } else if file.lines == 0 && file.size == 0 {
        out.push_str("The file is empty.\n");
    } else {
        write_lines(out, file);
    }
    write!(out, "</{tag}>").unwrap();
}
fn write_lines(out: &mut String, file: &LinkedFile) {
    let size = format_bytes(file.size);
    if file.from == 0 && file.lines > 0 {
        writeln!(
            out,
            "No lines shown: the file has {} ({size}).",
            count(file.lines, "line", "lines")
        )
        .unwrap();
    } else if file.from == 0 {
        writeln!(out, "No lines shown: the file is {size}.").unwrap();
    } else if file.from == 1 && file.to == file.lines {
        writeln!(out, "All {} ({size}):", count(file.lines, "line", "lines")).unwrap();
    } else if file.lines > 0 {
        writeln!(
            out,
            "{} of {} ({size}):",
            upper(&line_range(file.from, file.to)),
            file.lines
        )
        .unwrap();
    } else {
        writeln!(
            out,
            "{} of a file of {size}:",
            upper(&line_range(file.from, file.to))
        )
        .unwrap();
    }
    if file.from > 0 {
        for (index, line) in lines(&file.content).enumerate() {
            writeln!(out, "{:6}\t{line}", file.from + index as i64).unwrap();
        }
    }
    let mut missing = Vec::new();
    if file.from > 1 {
        missing.push(line_range(1, file.from - 1));
    }
    if file.to > 0 && file.lines > file.to {
        missing.push(line_range(file.to + 1, file.lines));
    } else if file.to > 0 && file.lines == 0 {
        missing.push(format!("the lines after line {}", file.to));
    }
    let mut from = file.to + 1;
    let mut to = file.to + 1000;
    if file.from == 0 {
        to = if file.lines > 0 {
            file.lines.min(1000)
        } else {
            1000
        };
        writeln!(
            out,
            "Read it from the file when you need it, e.g. `sed -n '1,{to}p' {}`.",
            shell_quote(&file.path)
        )
        .unwrap();
        return;
    }
    if missing.is_empty() {
        return;
    }
    if file.lines > file.to {
        to = to.min(file.lines);
    } else if file.lines > 0 {
        from = 1.max(file.from - 1000);
        to = file.from - 1;
    }
    writeln!(out, "Not shown: {}. Read them from the file when you need them, e.g. `sed -n '{from},{to}p' {}`.", missing.join(" and "), shell_quote(&file.path)).unwrap();
}
fn write_folder(out: &mut String, file: &LinkedFile) {
    if file.lines == 0 {
        out.push_str("The folder is empty.\n");
        return;
    }
    if file.from == 1 && file.to == file.lines {
        writeln!(out, "{}:", upper(&count(file.lines, "entry", "entries"))).unwrap();
    } else {
        writeln!(out, "Entries {}-{} of {}:", file.from, file.to, file.lines).unwrap();
    }
    for line in lines(&file.content) {
        writeln!(out, "{line}").unwrap();
    }
    if file.lines > file.to {
        writeln!(
            out,
            "Not shown: {} more; list them with `ls -A {}`.",
            file.lines - file.to,
            shell_quote(&file.path)
        )
        .unwrap();
    }
}
fn lines(text: &str) -> impl Iterator<Item = &str> {
    text.strip_suffix('\n')
        .unwrap_or(text)
        .split('\n')
        .filter(move |_| !text.is_empty())
}
fn line_range(from: i64, to: i64) -> String {
    if from == to {
        format!("line {from}")
    } else {
        format!("lines {from}-{to}")
    }
}
fn count(n: i64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}
fn upper(text: &str) -> String {
    let mut out = text.to_owned();
    if let Some(first) = out.get_mut(..1) {
        first.make_ascii_uppercase();
    }
    out
}
fn format_bytes(n: i64) -> String {
    if n < 1 << 10 {
        count(n, "byte", "bytes")
    } else if n < 1 << 20 {
        format!("{:.1} KB", n as f64 / (1 << 10) as f64)
    } else if n < 1 << 30 {
        format!("{:.1} MB", n as f64 / (1 << 20) as f64)
    } else {
        format!("{:.1} GB", n as f64 / (1 << 30) as f64)
    }
}
fn attribute(text: &str) -> String {
    format!(
        "\"{}\"",
        text.replace('&', "&amp;")
            .replace('"', "&quot;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    )
}
fn shell_quote(text: &str) -> String {
    if !text.is_empty()
        && !text.starts_with(['-', '~'])
        && text
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || b"_./@%+=:,~-".contains(&ch))
    {
        return text.into();
    }
    format!("'{}'", text.replace('\'', "'\\''"))
}
pub(super) fn answered_text(message: &ExternalMessage) -> String {
    let mut text = message.text.clone();
    if !message.images.is_empty() {
        let labels: Vec<String> = message
            .images
            .iter()
            .enumerate()
            .map(|(index, image)| {
                if image.label.is_empty() {
                    format!("image {}", index + 1)
                } else {
                    image.label.clone()
                }
            })
            .collect();
        write!(
            text,
            "\n[The user attached {} here; images are left out after a compaction.]",
            labels.join(", ")
        )
        .unwrap();
    }
    if !message.files.is_empty() {
        let labels: Vec<String> = message
            .files
            .iter()
            .map(|file| {
                if file.label.is_empty() {
                    format!("${}", file.path)
                } else {
                    file.label.clone()
                }
            })
            .collect();
        write!(text, "\n[The user linked {} here; what they showed is left out after a compaction: read the files themselves.]", labels.join(", ")).unwrap();
    }
    text
}
