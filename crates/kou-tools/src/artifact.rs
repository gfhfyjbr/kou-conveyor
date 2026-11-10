//! Descriptor-relative, non-destructive artifact creation and bounded capture reads.
use crate::process::CancelToken;
use rustix::{
    fs::{AtFlags, FileType, Mode, OFlags, fsync, mkdirat, openat, statat},
    io::Errno,
};
use std::{
    ffi::OsString,
    fs::{File, OpenOptions},
    io,
    os::unix::{
        ffi::{OsStrExt, OsStringExt},
        fs::{FileExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    File,
    Directory,
}
#[derive(Debug)]
pub struct Created {
    pub kind: Kind,
    pub created: bool,
}
#[derive(Debug)]
pub struct CapturedRead {
    pub size: i64,
    pub read: i64,
}
pub const MAX_CAPTURE_READ: i64 = 4_000_000;

fn canceled(cancel: &CancelToken) -> io::Result<()> {
    if cancel.is_canceled() {
        Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "context canceled",
        ))
    } else {
        Ok(())
    }
}
fn retry<T>(mut call: impl FnMut() -> rustix::io::Result<T>) -> rustix::io::Result<T> {
    loop {
        match call() {
            Err(Errno::INTR) => {}
            result => return result,
        }
    }
}
fn parts(path: &Path, kind: Kind) -> io::Result<(PathBuf, OsString)> {
    let mut bytes = path.as_os_str().as_bytes();
    if bytes.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "path is empty"));
    }
    if bytes.ends_with(b"/") {
        if kind != Kind::Directory {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "file path has a trailing separator",
            ));
        }
        while bytes.len() > 1 && bytes.ends_with(b"/") {
            bytes = &bytes[..bytes.len() - 1];
        }
    }
    let (parent, name) = match bytes.iter().rposition(|&byte| byte == b'/') {
        None => (b".".as_slice(), bytes),
        Some(0) => (b"/".as_slice(), &bytes[1..]),
        Some(index) => (&bytes[..index], &bytes[index + 1..]),
    };
    if matches!(name, b"" | b"." | b"..") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "path does not name a creatable entry",
        ));
    }
    Ok((
        PathBuf::from(OsString::from_vec(parent.to_vec())),
        OsString::from_vec(name.to_vec()),
    ))
}
fn sync_parent(parent: &File) -> io::Result<()> {
    retry(|| fsync(parent))?;
    #[cfg(target_vendor = "apple")]
    match retry(|| rustix::fs::fcntl_fullfsync(parent)) {
        Ok(()) | Err(Errno::NOTSUP) => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

/// Mode is a Unix permission/special-bit mask, not Go's os.FileMode encoding.
/// Reusing an existing entry never opens/truncates/chmods it. The parent stays
/// pinned across create/stat retries, including replay of mode-000 artifacts.
pub fn create(path: &Path, kind: Kind, mode: u32, cancel: &CancelToken) -> io::Result<Created> {
    if mode & !0o7777 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "unsupported Unix mode bits",
        ));
    }
    let (parent_path, name) = parts(path, kind)?;
    canceled(cancel)?;
    let parent = OpenOptions::new()
        .read(true)
        .custom_flags((OFlags::DIRECTORY | OFlags::CLOEXEC).bits() as _)
        .open(parent_path)?;
    let mode = Mode::from_bits_truncate(mode as _);
    for _ in 0..16 {
        canceled(cancel)?;
        let created = match kind {
            Kind::File => match retry(|| {
                openat(
                    &parent,
                    &name,
                    OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC,
                    mode,
                )
            }) {
                Ok(fd) => {
                    let file = File::from(fd);
                    retry(|| fsync(&file))?;
                    sync_parent(&parent)?;
                    return Ok(Created {
                        kind,
                        created: true,
                    });
                }
                Err(error) => Err(error),
            },
            Kind::Directory => retry(|| mkdirat(&parent, &name, mode)),
        };
        match created {
            Ok(()) => {
                sync_parent(&parent)?;
                return Ok(Created {
                    kind,
                    created: true,
                });
            }
            Err(Errno::EXIST) => {}
            Err(error) => return Err(error.into()),
        }
        let stat = match retry(|| statat(&parent, &name, AtFlags::SYMLINK_NOFOLLOW)) {
            Err(Errno::NOENT) => continue,
            result => result?,
        };
        let expected = if kind == Kind::File {
            FileType::RegularFile
        } else {
            FileType::Directory
        };
        if FileType::from_raw_mode(stat.st_mode) != expected {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                if kind == Kind::File {
                    "existing path is not a regular file"
                } else {
                    "existing path is not a directory"
                },
            ));
        }
        sync_parent(&parent)?;
        return Ok(Created {
            kind,
            created: false,
        });
    }
    Err(io::Error::other(
        "path changed while creating the filesystem entry",
    ))
}

/// Private capture policy: regular files only, no leaf symlinks, at most 4 MB per
/// call. Unlike the general Go ReadFile primitive this never waits on a FIFO or
/// device. Each callback borrows at most 8 KiB; the backend retains no output body.
pub fn read_capture(
    path: &Path,
    offset: i64,
    count: i64,
    cancel: &CancelToken,
    mut on_chunk: impl FnMut(i64, &[u8]) -> io::Result<()>,
) -> io::Result<CapturedRead> {
    if offset < 0 || !(0..=MAX_CAPTURE_READ).contains(&count) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "capture read offset/count is out of range",
        ));
    }
    canceled(cancel)?;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags((OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC).bits() as _)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "capture path is not a regular file",
        ));
    }
    let mut buffer = [0u8; 8192];
    let mut read = 0i64;
    while read < count {
        canceled(cancel)?;
        let at = offset.checked_add(read).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "capture read offset overflow")
        })?;
        let capacity = buffer.len().min((count - read) as usize);
        let size = loop {
            match file.read_at(&mut buffer[..capacity], at as u64) {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                result => break result?,
            }
        };
        if size == 0 {
            break;
        }
        canceled(cancel)?;
        on_chunk(at, &buffer[..size])?;
        read += size as i64;
    }
    canceled(cancel)?;
    let size = i64::try_from(file.metadata()?.len())
        .map_err(|_| io::Error::other("capture size is out of range"))?;
    Ok(CapturedRead { size, read })
}
