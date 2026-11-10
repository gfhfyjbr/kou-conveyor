use kou_tools::{
    artifact::{self, Kind},
    process::CancelToken,
};
use std::{
    fs, io,
    os::unix::{
        ffi::OsStrExt,
        fs::{FileTypeExt, PermissionsExt, symlink},
    },
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Arc, Barrier},
    time::{Duration, Instant},
};

#[test]
fn creates_syncs_and_reuses_files_and_directories_without_mutation() {
    let root = tempfile::tempdir().unwrap();
    let cancel = CancelToken::default();
    for (name, kind, mode) in [
        ("file", Kind::File, 0o600),
        ("directory", Kind::Directory, 0o700),
    ] {
        let path = root.path().join(name);
        let first = artifact::create(&path, kind, mode, &cancel).unwrap();
        assert_eq!(first.kind, kind);
        assert!(first.created);
        if kind == Kind::File {
            fs::write(&path, b"keep bytes").unwrap();
        }
        let again = artifact::create(&path, kind, 0, &cancel).unwrap();
        assert!(!again.created);
        assert_eq!(again.kind, kind);
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            mode
        );
        if kind == Kind::File {
            assert_eq!(fs::read(path).unwrap(), b"keep bytes");
        }
    }
    let raw_name = root.path().join(std::ffi::OsStr::from_bytes(b"raw-\xff"));
    let created = artifact::create(&raw_name, Kind::File, 0o600, &cancel);
    let control = fs::OpenOptions::new().write(true).create_new(true).open(
        root.path()
            .join(std::ffi::OsStr::from_bytes(b"control-\xff")),
    );
    match control {
        Ok(_) => assert!(created.unwrap().created),
        // APFS rejects non-UTF-8 names; preserve the actual filesystem error.
        Err(error) => assert_eq!(created.unwrap_err().raw_os_error(), error.raw_os_error()),
    }
}

#[test]
fn restrictive_and_zero_mode_entries_are_replayable_without_opening_or_chmod() {
    let root = tempfile::tempdir().unwrap();
    let cancel = CancelToken::default();
    for (name, kind) in [("zero-file", Kind::File), ("zero-dir", Kind::Directory)] {
        let path = root.path().join(name);
        assert!(artifact::create(&path, kind, 0, &cancel).unwrap().created);
        assert!(
            !artifact::create(&path, kind, 0o700, &cancel)
                .unwrap()
                .created
        );
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0);
        if kind == Kind::Directory {
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        }
    }
    let path = root.path().join("readonly");
    fs::write(&path, b"untouched").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o400)).unwrap();
    assert!(
        !artifact::create(&path, Kind::File, 0o600, &cancel)
            .unwrap()
            .created
    );
    assert_eq!(fs::read(&path).unwrap(), b"untouched");
    assert_eq!(
        fs::metadata(path).unwrap().permissions().mode() & 0o777,
        0o400
    );
}

#[test]
fn wrong_kinds_symlinks_and_fifos_are_never_followed_or_replaced() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("target");
    fs::write(&target, b"keep").unwrap();
    let link = root.path().join("link");
    symlink(&target, &link).unwrap();
    let fifo = root.path().join("fifo");
    assert!(
        std::process::Command::new("/usr/bin/mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    let cancel = CancelToken::default();
    for (path, kind) in [
        (&link, Kind::File),
        (&link, Kind::Directory),
        (&fifo, Kind::File),
        (&target, Kind::Directory),
        (&root.path().to_owned(), Kind::File),
    ] {
        assert_eq!(
            artifact::create(path, kind, 0o600, &cancel)
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
    }
    assert_eq!(fs::read(&target).unwrap(), b"keep");
    assert_eq!(fs::read_link(&link).unwrap(), target);
    assert!(fs::symlink_metadata(fifo).unwrap().file_type().is_fifo());
}

#[test]
fn path_resolution_relative_paths_and_trailing_directory_separators_are_preserved() {
    let root = tempfile::tempdir().unwrap();
    let cancel = CancelToken::default();
    let target = root.path().join("target");
    fs::create_dir(&target).unwrap();
    let alias = root.path().join("alias");
    symlink(&target, &alias).unwrap();
    assert!(
        artifact::create(&alias.join("file"), Kind::File, 0o600, &cancel)
            .unwrap()
            .created
    );
    assert!(target.join("file").is_file());
    let unresolved = root.path().join("missing/../must-not-clean");
    assert!(artifact::create(&unresolved, Kind::Directory, 0o700, &cancel).is_err());
    assert!(!root.path().join("must-not-clean").exists());
    let trailing = root.path().join("trailing///");
    assert!(
        artifact::create(&trailing, Kind::Directory, 0o700, &cancel)
            .unwrap()
            .created
    );
    assert!(root.path().join("trailing").is_dir());
    for path in [
        std::path::Path::new(""),
        std::path::Path::new("/"),
        std::path::Path::new("."),
        std::path::Path::new(".."),
        std::path::Path::new("file/"),
    ] {
        assert_eq!(
            artifact::create(path, Kind::File, 0o600, &cancel)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
    }
    assert!(
        artifact::create(
            &root.path().join("invalid-mode"),
            Kind::File,
            0x8000,
            &cancel
        )
        .is_err()
    );
    assert!(!root.path().join("invalid-mode").exists());
    let cwd = std::env::current_dir().unwrap();
    let relative_root = tempfile::tempdir_in(&cwd).unwrap();
    let relative = relative_root
        .path()
        .strip_prefix(&cwd)
        .unwrap()
        .join("relative");
    assert!(
        artifact::create(&relative, Kind::File, 0o600, &cancel)
            .unwrap()
            .created
    );
    assert!(relative_root.path().join("relative").is_file());
}

#[test]
fn concurrent_replay_reports_exactly_one_creation_without_changing_contents() {
    let root = tempfile::tempdir().unwrap();
    for kind in [Kind::File, Kind::Directory] {
        let path = root.path().join(if kind == Kind::File {
            "file"
        } else {
            "directory"
        });
        let gate = Arc::new(Barrier::new(24));
        let results = std::thread::scope(|scope| {
            let jobs: Vec<_> = (0..24)
                .map(|_| {
                    let gate = gate.clone();
                    let path = &path;
                    scope.spawn(move || {
                        gate.wait();
                        artifact::create(path, kind, 0o700, &CancelToken::default()).unwrap()
                    })
                })
                .collect();
            jobs.into_iter()
                .map(|job| job.join().unwrap())
                .collect::<Vec<_>>()
        });
        assert_eq!(results.iter().filter(|result| result.created).count(), 1);
        assert!(results.iter().all(|result| result.kind == kind));
        if kind == Kind::File {
            assert!(fs::read(&path).unwrap().is_empty());
        }
    }
}

#[test]
fn capture_reads_use_bounded_borrowed_chunks_and_report_full_file_size() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("out");
    let bytes: Vec<u8> = (0..50000).map(|index| (index % 256) as u8).collect();
    fs::write(&path, &bytes).unwrap();
    for (offset, count) in [(37, 19000), (49000, 5000), (50001, 100), (0, 0)] {
        let mut output = Vec::new();
        let result = artifact::read_capture(
            &path,
            offset,
            count,
            &CancelToken::default(),
            |at, chunk| {
                assert_eq!(at, offset + output.len() as i64);
                assert!(!chunk.is_empty() && chunk.len() <= 8192);
                output.extend_from_slice(chunk);
                Ok(())
            },
        )
        .unwrap();
        let start = (offset as usize).min(bytes.len());
        let end = (offset as usize + count as usize).min(bytes.len());
        assert_eq!(output, bytes[start..end]);
        assert_eq!(result.size, bytes.len() as i64);
        assert_eq!(result.read, output.len() as i64);
    }
}

#[test]
fn capture_reads_reject_nonregular_paths_and_invalid_ranges_without_blocking() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("out");
    fs::write(&file, "keep").unwrap();
    let link = root.path().join("link");
    symlink(&file, &link).unwrap();
    let fifo = root.path().join("fifo");
    assert!(
        std::process::Command::new("/usr/bin/mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    let start = Instant::now();
    for path in [&link, &fifo, &root.path().to_owned()] {
        assert!(
            artifact::read_capture(path, 0, 10, &CancelToken::default(), |_, _| panic!(
                "invalid capture callback"
            ))
            .is_err()
        );
    }
    for (offset, count) in [(-1, 1), (0, -1), (0, artifact::MAX_CAPTURE_READ + 1)] {
        assert!(
            artifact::read_capture(
                &file,
                offset,
                count,
                &CancelToken::default(),
                |_, _| panic!("invalid range callback")
            )
            .is_err()
        );
    }
    assert!(start.elapsed() < Duration::from_secs(2));
    assert_eq!(fs::read(file).unwrap(), b"keep");
}

#[test]
fn cancellation_and_callback_failures_close_owned_resources_and_stop_delivery() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("out");
    let cancel = CancelToken::default();
    cancel.cancel();
    assert_eq!(
        artifact::create(&path, Kind::File, 0o600, &cancel)
            .unwrap_err()
            .kind(),
        io::ErrorKind::Interrupted
    );
    assert!(!path.exists());
    fs::write(&path, vec![b'x'; 20000]).unwrap();
    assert_eq!(
        artifact::read_capture(&path, 0, 20000, &cancel, |_, _| panic!("canceled callback"))
            .unwrap_err()
            .kind(),
        io::ErrorKind::Interrupted
    );
    let cancel = CancelToken::default();
    let mut calls = 0;
    let error = artifact::read_capture(&path, 0, 20000, &cancel, |_, _| {
        calls += 1;
        cancel.cancel();
        Ok(())
    })
    .unwrap_err();
    assert_eq!(calls, 1);
    assert_eq!(error.kind(), io::ErrorKind::Interrupted);
    let error = artifact::read_capture(&path, 0, 20000, &CancelToken::default(), |_, _| {
        Err(io::Error::other("callback refusal"))
    })
    .unwrap_err();
    assert_eq!(error.to_string(), "callback refusal");
    let panicked = catch_unwind(AssertUnwindSafe(|| {
        artifact::read_capture(&path, 0, 20000, &CancelToken::default(), |_, _| {
            panic!("callback panic")
        })
    }));
    assert!(panicked.is_err());
    assert_eq!(
        artifact::read_capture(&path, 0, 20000, &CancelToken::default(), |_, _| Ok(()))
            .unwrap()
            .read,
        20000
    );
}
