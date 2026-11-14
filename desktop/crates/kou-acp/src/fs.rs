//! The files an agent reads and writes through the client: `fs/*`.

use crate::rpc::{self, RpcError};
use std::{fs, io, path::Path};

/// `fs/read_text_file`: the whole file, or `limit` lines from the one-based `line`.
pub(crate) fn read_text_file(
    path: &Path,
    line: Option<u32>,
    limit: Option<u32>,
) -> Result<String, RpcError> {
    absolute(path)?;
    let bytes = fs::read(path).map_err(|error| failure("read", path, &error))?;
    // A lossy conversion would hand the agent text it then writes back damaged.
    let text = String::from_utf8(bytes).map_err(|_| {
        RpcError::new(
            rpc::INVALID_PARAMS,
            format!("{} is not UTF-8 text", path.display()),
        )
    })?;
    if line.is_none() && limit.is_none() {
        return Ok(text);
    }
    let skip = line.unwrap_or(1).saturating_sub(1) as usize;
    let take = limit.map_or(usize::MAX, |limit| limit as usize);
    Ok(text.split_inclusive('\n').skip(skip).take(take).collect())
}

/// `fs/write_text_file`: create or replace the file, and the folders above it.
pub(crate) fn write_text_file(path: &Path, content: &str) -> Result<(), RpcError> {
    absolute(path)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| failure("create the folder of", path, &error))?;
    }
    fs::write(path, content).map_err(|error| failure("write", path, &error))
}

fn absolute(path: &Path) -> Result<(), RpcError> {
    if path.is_absolute() {
        return Ok(());
    }
    Err(RpcError::new(
        rpc::INVALID_PARAMS,
        format!("the path must be absolute, got {}", path.display()),
    ))
}

fn failure(action: &str, path: &Path, error: &io::Error) -> RpcError {
    let code = match error.kind() {
        io::ErrorKind::NotFound => rpc::RESOURCE_NOT_FOUND,
        _ => rpc::INTERNAL_ERROR,
    };
    RpcError::new(
        code,
        format!("could not {action} {}: {error}", path.display()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_read_takes_a_window_of_lines() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("lines.txt");
        fs::write(&path, "one\ntwo\nthree\nfour").unwrap();
        let read = |line, limit| read_text_file(&path, line, limit).unwrap();
        assert_eq!(read(None, None), "one\ntwo\nthree\nfour");
        assert_eq!(read(Some(2), None), "two\nthree\nfour");
        assert_eq!(read(Some(2), Some(2)), "two\nthree\n");
        assert_eq!(read(None, Some(1)), "one\n");
        assert_eq!(read(Some(4), Some(5)), "four");
        assert_eq!(read(Some(9), None), "");
        // Line numbers are one-based; zero is read as the start.
        assert_eq!(read(Some(0), Some(1)), "one\n");
    }

    #[test]
    fn a_read_says_what_is_wrong_with_the_file() {
        let folder = tempfile::tempdir().unwrap();
        let missing = read_text_file(&folder.path().join("none.txt"), None, None).unwrap_err();
        assert_eq!(missing.code, rpc::RESOURCE_NOT_FOUND);
        let relative = read_text_file(Path::new("relative.txt"), None, None).unwrap_err();
        assert_eq!(relative.code, rpc::INVALID_PARAMS);
        let binary = folder.path().join("binary");
        fs::write(&binary, [0xff, 0xfe, 0x00]).unwrap();
        assert_eq!(
            read_text_file(&binary, None, None).unwrap_err().code,
            rpc::INVALID_PARAMS
        );
    }

    #[test]
    fn a_write_creates_the_file_and_its_folders() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("a/b/new.txt");
        write_text_file(&path, "first").unwrap();
        write_text_file(&path, "second").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "second");
        assert_eq!(
            write_text_file(Path::new("relative.txt"), "x")
                .unwrap_err()
                .code,
            rpc::INVALID_PARAMS
        );
        // A folder is in the way of the file.
        assert_eq!(
            write_text_file(&folder.path().join("a/b"), "x")
                .unwrap_err()
                .code,
            rpc::INTERNAL_ERROR
        );
    }
}
