use kou_tools::{fileops, patch::apply_patch};
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    sync::{Arc, Barrier},
};

fn patch(body: &str) -> String {
    format!("*** Begin Patch\n{body}*** End Patch")
}

#[test]
fn staged_add_update_move_delete_and_summary() {
    let root = tempfile::tempdir().unwrap();
    let result = apply_patch(&patch("*** Add File: first\n+one\n+two\n*** Update File: first\n@@\n-two\n+second\n*** Update File: first\n*** Move to: deep/renamed\n@@\n-one\n+1\n*** Add File: transient\n+gone\n*** Delete File: transient\n"), root.path()).unwrap();
    assert_eq!(
        fs::read(root.path().join("deep/renamed")).unwrap(),
        b"1\nsecond\n"
    );
    assert!(!root.path().join("first").exists());
    assert!(!root.path().join("transient").exists());
    assert_eq!(
        result.summary(),
        "Applied the patch:\nA first\nA transient\nR first -> deep/renamed\nD transient"
    );
    assert_eq!(
        result.files(),
        ["first", "transient", "transient", "deep/renamed"]
    );
}

#[test]
fn failed_validation_publishes_nothing_and_keeps_permissions() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("existing");
    fs::write(&path, "one\ntwo\n").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o751)).unwrap();
    let error = apply_patch(
        &patch("*** Add File: new\n+x\n*** Update File: existing\n@@\n-missing\n+z\n"),
        root.path(),
    )
    .unwrap_err();
    assert!(error.0.contains("context was not found"));
    assert_eq!(fs::read(&path).unwrap(), b"one\ntwo\n");
    assert!(!root.path().join("new").exists());
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o751
    );
}

#[test]
fn windows_endings_standalone_cr_and_invalid_bytes_survive() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("file");
    fs::write(&path, b"\xff\xf0\x90\r\none\r\ntail\r").unwrap();
    apply_patch(
        &patch("*** Update File: file\n@@\n-one\n+two\n"),
        root.path(),
    )
    .unwrap();
    assert_eq!(
        fs::read(&path).unwrap(),
        b"\xff\xf0\x90\r\ntwo\r\ntail\r\r\n"
    );
}

#[test]
fn whitespace_fallback_nested_headers_and_eof_choose_the_right_region() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("file");
    fs::write(
        &path,
        "class One\n  fn two\n    old  \nclass Three\n  fn four\n    old  \n",
    )
    .unwrap();
    apply_patch(
        &patch("*** Update File: file\n@@ class Three\n@@ fn four\n-old\n+new\n*** End of File\n"),
        root.path(),
    )
    .unwrap();
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "class One\n  fn two\n    old  \nclass Three\n  fn four\nnew\n"
    );
}

#[test]
fn malformed_empty_and_conflicting_patches_return_errors_without_panicking() {
    let root = tempfile::tempdir().unwrap();
    for body in [
        "garbage",
        "*** Begin Patch",
        "*** Begin Patch\n*** End Patch",
        "*** Begin Patch\n\n*** End Patch",
        "*** Begin Patch\n*** Add File: \n+x\n*** End Patch",
        "*** Begin Patch\n*** Add File: new\nnot prefixed\n*** End Patch",
    ] {
        assert!(apply_patch(body, root.path()).is_err(), "{body}");
    }
    fs::write(root.path().join("file"), "one\n").unwrap();
    assert!(
        apply_patch(&patch("*** Add File: file\n+x\n"), root.path())
            .unwrap_err()
            .0
            .contains("it exists")
    );
    assert!(
        apply_patch(&patch("*** Update File: file\n"), root.path())
            .unwrap_err()
            .0
            .contains("changes nothing")
    );
    assert!(
        apply_patch(&patch("*** Delete File: missing\n"), root.path())
            .unwrap_err()
            .0
            .contains("does not exist")
    );
}

#[test]
fn patches_and_edits_share_alias_locks_without_losing_changes() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("file");
    fs::write(
        &path,
        (0..24).map(|n| format!("line {n}\n")).collect::<String>(),
    )
    .unwrap();
    symlink("file", root.path().join("link")).unwrap();
    let barrier = Arc::new(Barrier::new(24));
    std::thread::scope(|scope| {
        for n in 0..24 {
            let root = root.path();
            let barrier = barrier.clone();
            scope.spawn(move || {
                barrier.wait();
                if n % 2 == 0 {
                    apply_patch(
                        &patch(&format!(
                            "*** Update File: link\n@@\n-line {n}\n+line {n} changed\n"
                        )),
                        root,
                    )
                    .unwrap();
                } else {
                    fileops::edit(
                        root.join("file"),
                        &format!("line {n}\n"),
                        &format!("line {n} changed\n"),
                        Default::default(),
                    )
                    .unwrap();
                }
            });
        }
    });
    let content = fs::read_to_string(&path).unwrap();
    for n in 0..24 {
        assert!(content.contains(&format!("line {n} changed\n")));
    }
    assert!(
        fs::symlink_metadata(root.path().join("link"))
            .unwrap()
            .is_symlink()
    );
}
