use kou_tools::fileops::{self, EditOptions, MAX_EDIT_BYTES, MAX_EDIT_RESULT_BYTES, ReadOptions};
use std::{
    fs,
    io::{Seek, SeekFrom, Write},
    os::unix::fs::{PermissionsExt, symlink},
    sync::{Arc, Barrier},
};

fn fixture(content: impl AsRef<[u8]>) -> (tempfile::TempDir, std::path::PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("file");
    fs::write(&path, content).unwrap();
    (root, path)
}

#[test]
fn numbered_lines_offsets_limits_and_byte_footer() {
    let (_root, path) = fixture("1\n2\n3\n4\n5");
    let result = fileops::read(
        &path,
        ReadOptions {
            offset: 2,
            limit: 2,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        result.text,
        "     2\t2\n     3\t3\n[Lines 2-3 of 5 shown; read on with offset=4.]"
    );
    assert_eq!(
        (
            result.lines,
            result.from,
            result.to,
            result.size,
            result.truncated
        ),
        (5, 2, 3, 9, true)
    );
    assert_eq!(
        fileops::read(
            &path,
            ReadOptions {
                offset: 4,
                ..Default::default()
            }
        )
        .unwrap()
        .text,
        "     4\t4\n     5\t5\n[Lines 4-5 of 5 shown.]"
    );
    assert!(
        fileops::read(
            &path,
            ReadOptions {
                offset: 9,
                ..Default::default()
            }
        )
        .unwrap_err()
        .0
        .contains("past the end")
    );
    assert!(fileops::read(&path, ReadOptions { max_bytes: 1, ..Default::default() }).unwrap().text.ends_with("[Lines 1-1 of 5 shown, about 1 bytes, the most one read shows; read on with offset=2, or find what you need with rg -n.]"));
}

#[test]
fn read_empty_crlf_and_unicode_boundaries() {
    let (_root, path) = fixture("");
    let result = fileops::read(&path, ReadOptions::default()).unwrap();
    assert_eq!(result.text, "(empty file)");
    assert_eq!((result.lines, result.from, result.to), (0, 0, 0));
    fs::write(&path, "\r\n中中\r\nx\r").unwrap();
    let result = fileops::read(
        &path,
        ReadOptions {
            max_line_bytes: 4,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        result.text,
        "     1\t\n     2\t中 … [line cut: 3 more bytes]\n     3\tx"
    );
    assert_eq!(result.lines, 3);
}

#[test]
fn read_counts_giant_lines_without_materializing_their_tails() {
    let (_root, path) = fixture("");
    let mut file = fs::OpenOptions::new().write(true).open(&path).unwrap();
    let chunk = vec![b'x'; 64 * 1024];
    for _ in 0..1024 {
        file.write_all(&chunk).unwrap();
    }
    file.write_all(b"\r\nend\n").unwrap();
    drop(file);
    let result = fileops::read(
        &path,
        ReadOptions {
            max_line_bytes: 10,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        result.text,
        "     1\txxxxxxxxxx … [line cut: 67108854 more bytes]\n     2\tend"
    );
    assert_eq!(result.lines, 2);
    assert_eq!(
        fileops::read(
            &path,
            ReadOptions {
                offset: 2,
                ..Default::default()
            }
        )
        .unwrap()
        .text,
        "     2\tend\n[Lines 2-2 of 2 shown.]"
    );
}

#[test]
fn read_binary_head_image_hint_directory_and_options() {
    let (root, path) = fixture(b"PNG\0");
    let image = root.path().join("image.PNG");
    fs::rename(&path, &image).unwrap();
    assert!(
        fileops::read(&image, ReadOptions::default())
            .unwrap_err()
            .0
            .ends_with("; ViewImage shows images")
    );
    assert!(
        fileops::read(root.path(), ReadOptions::default())
            .unwrap_err()
            .0
            .contains("list it with Bash")
    );
    fs::write(&image, "text").unwrap();
    assert_eq!(
        fileops::read(
            &image,
            ReadOptions {
                offset: -1,
                ..Default::default()
            }
        )
        .unwrap_err()
        .0,
        "offset must not be negative"
    );
    assert_eq!(
        fileops::read(
            &image,
            ReadOptions {
                limit: -1,
                ..Default::default()
            }
        )
        .unwrap_err()
        .0,
        "limit must not be negative"
    );
    assert!(
        fileops::read(&path, ReadOptions::default())
            .unwrap_err()
            .0
            .ends_with("does not exist")
    );
}

#[test]
fn edits_exact_bytes_and_keeps_invalid_utf8_outside_replacement() {
    let (_root, path) = fixture(b"\xff\xf0\x90\nprefix\none\nsuffix\n");
    let result = fileops::edit(&path, "one", "two\nthree", EditOptions::default()).unwrap();
    assert_eq!(
        fs::read(&path).unwrap(),
        b"\xff\xf0\x90\nprefix\ntwo\nthree\nsuffix\n"
    );
    assert_eq!((result.replacements, result.line, result.lines), (1, 3, 5));
    assert!(result.snippet.starts_with("     1\t���\n"));
    assert!(result.snippet.contains("     3\ttwo\n     4\tthree"));
}

#[test]
fn ambiguity_nonoverlapping_matches_and_not_found_hints_are_nonmutating() {
    let (_root, path) = fixture("aaaa\nif x {\n    return\n}\n");
    assert!(
        fileops::edit(&path, "aa", "x", EditOptions::default())
            .unwrap_err()
            .0
            .contains("appears 2 times")
    );
    for (old, hint) in [
        ("if x {\n\treturn\n}", "whitespace is ignored"),
        ("if x {\n    return 5\n}", "first line is there"),
        ("nothing", "without the line numbers"),
        ("", "must not be empty"),
    ] {
        assert!(
            fileops::edit(&path, old, "x", EditOptions::default())
                .unwrap_err()
                .0
                .contains(hint)
        );
    }
    assert!(
        fileops::edit(&path, "same", "same", EditOptions::default())
            .unwrap_err()
            .0
            .contains("the same")
    );
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "aaaa\nif x {\n    return\n}\n"
    );
    let result = fileops::edit(&path, "aa", "x", EditOptions { replace_all: true }).unwrap();
    assert_eq!(result.replacements, 2);
    assert!(fs::read_to_string(&path).unwrap().starts_with("xx\n"));
}

#[test]
fn symlinks_crlf_and_permissions_survive_edit_and_write() {
    let (root, path) = fixture("echo one\r\necho two\r\n");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o751)).unwrap();
    let link = root.path().join("link");
    symlink("file", &link).unwrap();
    fileops::edit(
        &link,
        "echo one\necho two\n",
        "echo three\n",
        EditOptions::default(),
    )
    .unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"echo three\r\n");
    let result = fileops::write(&link, "last\n").unwrap();
    assert!(!result.created);
    assert_eq!(result.previous_lines, 1);
    assert_eq!(fs::read(&path).unwrap(), b"last\n");
    assert!(fs::symlink_metadata(&link).unwrap().is_symlink());
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o751
    );
}

#[test]
fn write_creates_directories_counts_and_replaces_whole() {
    let root = tempfile::tempdir().unwrap();
    let name = root.path().join("deep/dir/file");
    let result = fileops::write(&name, "one\ntwo\n").unwrap();
    assert_eq!(
        (
            result.bytes,
            result.lines,
            result.created,
            result.previous_lines
        ),
        (8, 2, true, 0)
    );
    assert_eq!(
        fs::metadata(&name).unwrap().permissions().mode() & 0o777,
        0o644
    );
    let result = fileops::write(&name, "three").unwrap();
    assert_eq!(
        (
            result.bytes,
            result.lines,
            result.created,
            result.previous_lines
        ),
        (5, 1, false, 2)
    );
    assert!(
        fileops::write(root.path(), "x")
            .unwrap_err()
            .0
            .contains("is a directory")
    );
    assert_eq!(
        fs::read_dir(name.parent().unwrap()).unwrap().count(),
        1,
        "no temporary artifacts"
    );
}

#[test]
fn edit_input_and_expansion_limits_reject_before_mutation() {
    let (_root, path) = fixture("x");
    let mut file = fs::OpenOptions::new().write(true).open(&path).unwrap();
    file.seek(SeekFrom::Start(MAX_EDIT_BYTES as u64)).unwrap();
    file.write_all(b"end").unwrap();
    drop(file);
    assert!(
        fileops::edit(&path, "x", "y", EditOptions::default())
            .unwrap_err()
            .0
            .contains("too large to edit")
    );
    fs::write(&path, "xx").unwrap();
    let replacement = "a".repeat(MAX_EDIT_RESULT_BYTES / 2 + 1);
    assert!(
        fileops::edit(&path, "x", &replacement, EditOptions { replace_all: true })
            .unwrap_err()
            .0
            .contains("result limit")
    );
    assert_eq!(fs::read(&path).unwrap(), b"xx");
}

#[test]
fn parallel_alias_edits_all_land() {
    let (root, path) = fixture((0..32).map(|n| format!("line {n}\n")).collect::<String>());
    let link = root.path().join("link");
    symlink("file", &link).unwrap();
    let barrier = Arc::new(Barrier::new(32));
    std::thread::scope(|scope| {
        for n in 0..32 {
            let path = if n % 2 == 0 { &path } else { &link };
            let barrier = barrier.clone();
            scope.spawn(move || {
                barrier.wait();
                fileops::edit(
                    path,
                    &format!("line {n}\n"),
                    &format!("line {n} edited\n"),
                    EditOptions::default(),
                )
                .unwrap();
            });
        }
    });
    let content = fs::read_to_string(&path).unwrap();
    for n in 0..32 {
        assert!(content.contains(&format!("line {n} edited\n")));
    }
}

#[test]
fn parallel_creation_via_directory_alias_never_reports_two_creations_or_mixes_bytes() {
    let root = tempfile::tempdir().unwrap();
    let real = root.path().join("real");
    fs::create_dir(&real).unwrap();
    let alias = root.path().join("alias");
    symlink(&real, &alias).unwrap();
    let barrier = Arc::new(Barrier::new(16));
    let created = std::thread::scope(|scope| {
        let mut jobs = Vec::new();
        for n in 0..16 {
            let path = if n % 2 == 0 { &real } else { &alias }.join("missing/deep/file");
            let barrier = barrier.clone();
            jobs.push(scope.spawn(move || {
                barrier.wait();
                fileops::write(path, vec![b'0' + n; 65536]).unwrap().created
            }));
        }
        jobs.into_iter()
            .map(|job| job.join().unwrap())
            .filter(|created| *created)
            .count()
    });
    assert_eq!(created, 1);
    let bytes = fs::read(real.join("missing/deep/file")).unwrap();
    assert_eq!(bytes.len(), 65536);
    assert!(bytes.iter().all(|b| *b == bytes[0]));
}
