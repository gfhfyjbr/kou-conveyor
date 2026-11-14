//! A line diff for showing a tool's change to a file: Myers' algorithm over
//! lines, then everything far from a change folded away.

/// What a diff line is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// In both texts.
    Context,
    /// Only in the new text.
    Added,
    /// Only in the old text.
    Removed,
}

/// One line of a diff with its one-based numbers in the old and the new text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Line {
    pub kind: Kind,
    pub old: Option<u32>,
    pub new: Option<u32>,
    pub text: String,
}

/// A row to draw: a line, or a count of unchanged lines left out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Row {
    Line(Line),
    Gap(usize),
}

/// A change to one file, ready to draw.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Diff {
    pub rows: Vec<Row>,
    /// Lines only in the new text.
    pub added: usize,
    /// Lines only in the old text.
    pub removed: usize,
}

/// Unchanged lines kept around each change.
pub(crate) const CONTEXT: usize = 3;

/// An edit script longer than this is not searched for; the texts are shown as
/// one removal and one addition instead. Bounds the search to a few megabytes.
const MAX_EDITS: usize = 1500;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Equal,
    Delete,
    Insert,
}

/// Diff `old` against `new` by lines, keeping `context` unchanged lines around
/// each change. `old` is `None` for a new file: every line is an addition.
pub(crate) fn diff(old: Option<&str>, new: &str, context: usize) -> Diff {
    let old_lines: Vec<&str> = old.map(|old| old.lines().collect()).unwrap_or_default();
    let new_lines: Vec<&str> = new.lines().collect();
    let ops = script(&old_lines, &new_lines);

    let mut lines = Vec::with_capacity(ops.len());
    let (mut old_at, mut new_at) = (0usize, 0usize);
    let (mut added, mut removed) = (0, 0);
    for op in ops {
        match op {
            Op::Equal => {
                lines.push(Line {
                    kind: Kind::Context,
                    old: Some(old_at as u32 + 1),
                    new: Some(new_at as u32 + 1),
                    text: new_lines[new_at].to_owned(),
                });
                old_at += 1;
                new_at += 1;
            }
            Op::Delete => {
                lines.push(Line {
                    kind: Kind::Removed,
                    old: Some(old_at as u32 + 1),
                    new: None,
                    text: old_lines[old_at].to_owned(),
                });
                old_at += 1;
                removed += 1;
            }
            Op::Insert => {
                lines.push(Line {
                    kind: Kind::Added,
                    old: None,
                    new: Some(new_at as u32 + 1),
                    text: new_lines[new_at].to_owned(),
                });
                new_at += 1;
                added += 1;
            }
        }
    }

    Diff {
        rows: fold(lines, context),
        added,
        removed,
    }
}

/// Replace runs of context far from any change with a gap.
fn fold(lines: Vec<Line>, context: usize) -> Vec<Row> {
    let changed: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.kind != Kind::Context)
        .map(|(index, _)| index)
        .collect();
    if changed.is_empty() {
        return Vec::new();
    }
    let mut keep = vec![false; lines.len()];
    for &index in &changed {
        let from = index.saturating_sub(context);
        let to = (index + context + 1).min(lines.len());
        keep[from..to].fill(true);
    }
    // A gap row costs a line itself: hiding one or two lines behind it loses more than it saves.
    let mut index = 0;
    while index < keep.len() {
        if keep[index] {
            index += 1;
            continue;
        }
        let end = keep[index..]
            .iter()
            .position(|kept| *kept)
            .map_or(keep.len(), |offset| index + offset);
        if end - index <= 2 {
            keep[index..end].fill(true);
        }
        index = end;
    }

    let mut rows = Vec::new();
    let mut hidden = 0;
    for (line, keep) in lines.into_iter().zip(keep) {
        if keep {
            if hidden > 0 {
                rows.push(Row::Gap(hidden));
                hidden = 0;
            }
            rows.push(Row::Line(line));
        } else {
            hidden += 1;
        }
    }
    if hidden > 0 {
        rows.push(Row::Gap(hidden));
    }
    rows
}

/// The edit script from `old` to `new`. Within one change, removals come first.
fn script(old: &[&str], new: &[&str]) -> Vec<Op> {
    // Edits are usually small next to the file: only the middle needs a search.
    let prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let suffix = old[prefix..]
        .iter()
        .rev()
        .zip(new[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let old_middle = &old[prefix..old.len() - suffix];
    let new_middle = &new[prefix..new.len() - suffix];

    let mut ops = vec![Op::Equal; prefix];
    let middle = myers(old_middle, new_middle).unwrap_or_else(|| {
        let mut ops = vec![Op::Delete; old_middle.len()];
        ops.resize(old_middle.len() + new_middle.len(), Op::Insert);
        ops
    });
    // Myers may interleave the two sides of one change; show the old lines, then the new.
    let mut index = 0;
    while index < middle.len() {
        if middle[index] == Op::Equal {
            ops.push(Op::Equal);
            index += 1;
            continue;
        }
        let end = middle[index..]
            .iter()
            .position(|op| *op == Op::Equal)
            .map_or(middle.len(), |offset| index + offset);
        let deletes = middle[index..end]
            .iter()
            .filter(|op| **op == Op::Delete)
            .count();
        ops.extend(std::iter::repeat_n(Op::Delete, deletes));
        ops.extend(std::iter::repeat_n(Op::Insert, end - index - deletes));
        index = end;
    }
    ops.resize(ops.len() + suffix, Op::Equal);
    ops
}

/// Myers' greedy shortest edit script, or `None` if it is longer than [`MAX_EDITS`].
fn myers(old: &[&str], new: &[&str]) -> Option<Vec<Op>> {
    let (n, m) = (old.len() as isize, new.len() as isize);
    if n == 0 || m == 0 {
        let mut ops = vec![Op::Delete; old.len()];
        ops.resize(old.len() + new.len(), Op::Insert);
        return Some(ops);
    }
    let limit = ((n + m) as usize).min(MAX_EDITS) as isize;
    let offset = limit + 1;
    // `reach[k + offset]` is the furthest x on diagonal k (x - y = k).
    let mut reach = vec![0isize; (2 * limit + 3) as usize];
    // The part of `reach` each round could read, saved to walk back through.
    let mut trace: Vec<Vec<isize>> = Vec::new();
    let mut found = None;

    'search: for d in 0..=limit {
        trace.push(reach[(offset - d - 1) as usize..=(offset + d + 1) as usize].to_vec());
        let mut k = -d;
        while k <= d {
            let index = (k + offset) as usize;
            let mut x = if k == -d || (k != d && reach[index - 1] < reach[index + 1]) {
                reach[index + 1]
            } else {
                reach[index - 1] + 1
            };
            let mut y = x - k;
            while x < n && y < m && old[x as usize] == new[y as usize] {
                x += 1;
                y += 1;
            }
            reach[index] = x;
            if x >= n && y >= m {
                found = Some(d);
                break 'search;
            }
            k += 2;
        }
    }

    let found = found?;
    let mut ops = Vec::with_capacity((n + m) as usize);
    let (mut x, mut y) = (n, m);
    for d in (0..=found).rev() {
        let saved = &trace[d as usize];
        // `saved[k + d + 1]` is the reach on diagonal k before round d.
        let at = |k: isize| saved[(k + d + 1) as usize];
        let k = x - y;
        let previous_k = if k == -d || (k != d && at(k - 1) < at(k + 1)) {
            k + 1
        } else {
            k - 1
        };
        let previous_x = at(previous_k);
        let previous_y = previous_x - previous_k;
        while x > previous_x && y > previous_y {
            ops.push(Op::Equal);
            x -= 1;
            y -= 1;
        }
        if d > 0 {
            ops.push(if x == previous_x {
                Op::Insert
            } else {
                Op::Delete
            });
        }
        x = previous_x;
        y = previous_y;
    }
    ops.reverse();
    Some(ops)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A compact rendering: `-3 old`, `+3 new`, ` 4 same`, `~12` for a gap.
    fn show(diff: &Diff) -> Vec<String> {
        diff.rows
            .iter()
            .map(|row| match row {
                Row::Gap(count) => format!("~{count}"),
                Row::Line(line) => match line.kind {
                    Kind::Context => format!(" {} {}", line.new.unwrap(), line.text),
                    Kind::Added => format!("+{} {}", line.new.unwrap(), line.text),
                    Kind::Removed => format!("-{} {}", line.old.unwrap(), line.text),
                },
            })
            .collect()
    }

    /// Replaying a diff without folding must give back both texts.
    fn replay(old: &str, new: &str) {
        let diff = diff(Some(old), new, usize::MAX / 4);
        let mut old_out = Vec::new();
        let mut new_out = Vec::new();
        for row in &diff.rows {
            let Row::Line(line) = row else {
                panic!("nothing is folded with unlimited context");
            };
            match line.kind {
                Kind::Context => {
                    old_out.push(line.text.as_str());
                    new_out.push(line.text.as_str());
                }
                Kind::Added => new_out.push(line.text.as_str()),
                Kind::Removed => old_out.push(line.text.as_str()),
            }
        }
        if diff.added + diff.removed > 0 {
            assert_eq!(
                old_out,
                old.lines().collect::<Vec<_>>(),
                "old side of {old:?} -> {new:?}"
            );
            assert_eq!(
                new_out,
                new.lines().collect::<Vec<_>>(),
                "new side of {old:?} -> {new:?}"
            );
        } else {
            assert_eq!(
                old.lines().collect::<Vec<_>>(),
                new.lines().collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn a_changed_line_is_a_removal_then_an_addition_with_numbers() {
        let old = "[branch]\nremote = \"origin\"\ncheck = \"old\"\npush = \"allow\"\n";
        let new = "[branch]\nremote = \"origin\"\ncheck = \"new\"\npush = \"allow\"\n";
        let diff = diff(Some(old), new, CONTEXT);
        assert_eq!((diff.added, diff.removed), (1, 1));
        assert_eq!(
            show(&diff),
            [
                " 1 [branch]",
                " 2 remote = \"origin\"",
                "-3 check = \"old\"",
                "+3 check = \"new\"",
                " 4 push = \"allow\"",
            ]
        );
    }

    #[test]
    fn a_new_file_is_all_additions_and_an_emptied_one_all_removals() {
        let created = diff(None, "a\nb\n", CONTEXT);
        assert_eq!(show(&created), ["+1 a", "+2 b"]);
        assert_eq!((created.added, created.removed), (2, 0));
        let emptied = diff(Some("a\nb"), "", CONTEXT);
        assert_eq!(show(&emptied), ["-1 a", "-2 b"]);
        assert_eq!((emptied.added, emptied.removed), (0, 2));
    }

    #[test]
    fn identical_texts_have_no_rows() {
        let same = diff(Some("a\nb\nc"), "a\nb\nc\n", CONTEXT);
        assert_eq!(same, Diff::default());
    }

    #[test]
    fn long_unchanged_stretches_fold_but_short_ones_do_not() {
        let old: Vec<String> = (1..=40).map(|n| format!("line {n}")).collect();
        let mut new = old.clone();
        new[9] = "changed 10".into();
        new[16] = "changed 17".into();
        new.insert(30, "inserted".into());
        let diff = diff(Some(&old.join("\n")), &new.join("\n"), 2);
        assert_eq!(
            show(&diff),
            [
                "~7",
                " 8 line 8",
                " 9 line 9",
                "-10 line 10",
                "+10 changed 10",
                " 11 line 11",
                " 12 line 12",
                // Lines 13 and 14 sit between two kept stretches: shown, not folded.
                " 13 line 13",
                " 14 line 14",
                " 15 line 15",
                " 16 line 16",
                "-17 line 17",
                "+17 changed 17",
                " 18 line 18",
                " 19 line 19",
                "~9",
                " 29 line 29",
                " 30 line 30",
                "+31 inserted",
                " 32 line 31",
                " 33 line 32",
                "~8",
            ]
        );
        assert_eq!((diff.added, diff.removed), (3, 2));
    }

    #[test]
    fn one_change_shows_all_its_removals_before_its_additions() {
        let diff = diff(Some("a\nb\nc\nd"), "a\nx\ny\nz\nd", CONTEXT);
        assert_eq!(
            show(&diff),
            [" 1 a", "-2 b", "-3 c", "+2 x", "+3 y", "+4 z", " 5 d"]
        );
    }

    #[test]
    fn replaying_any_diff_restores_both_sides() {
        let cases = [
            ("", ""),
            ("a", ""),
            ("", "a"),
            ("a\nb\nc", "c\nb\na"),
            ("a\nb\nc\nd\ne", "a\nc\ne\nf"),
            ("x\ny\nz", "a\nx\nb\ny\nc\nz\nd"),
            ("same\nsame\nsame", "same\nsame"),
            ("one\ntwo\nthree\nfour", "zero\none\nthree\nfour\nfive"),
            ("a\r\nb\r\n", "a\nb\nc\n"),
        ];
        for (old, new) in cases {
            replay(old, new);
            replay(new, old);
        }
        // A pseudo-random pair, large enough to exercise the search itself.
        let mut seed = 0x2545_f491_4f6c_dd1d_u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let old: Vec<String> = (0..300).map(|_| format!("v{}", next() % 12)).collect();
        let mut new = Vec::new();
        for line in &old {
            if next() % 5 == 0 {
                continue;
            }
            new.push(line.clone());
            if next() % 7 == 0 {
                new.push(format!("n{}", next() % 12));
            }
        }
        replay(&old.join("\n"), &new.join("\n"));
    }

    #[test]
    fn a_rewrite_past_the_search_limit_is_still_a_correct_diff() {
        let old: Vec<String> = (0..MAX_EDITS + 10).map(|n| format!("old {n}")).collect();
        let new: Vec<String> = (0..MAX_EDITS + 10).map(|n| format!("new {n}")).collect();
        let (old, new) = (old.join("\n"), new.join("\n"));
        let diff = diff(Some(&old), &new, CONTEXT);
        assert_eq!((diff.added, diff.removed), (MAX_EDITS + 10, MAX_EDITS + 10));
        replay(&old, &new);
    }
}
