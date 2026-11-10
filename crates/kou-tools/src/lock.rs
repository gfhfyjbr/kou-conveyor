use std::{
    collections::HashMap,
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, Weak},
};

type Locks = Mutex<HashMap<PathBuf, Weak<Mutex<()>>>>;
static LOCKS: OnceLock<Locks> = OnceLock::new();

struct Cleanup;
impl Drop for Cleanup {
    fn drop(&mut self) {
        if let Some(registry) = LOCKS.get() {
            registry
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .retain(|_, value| value.strong_count() != 0);
        }
    }
}

pub(crate) fn with_paths<T>(paths: &[&Path], action: impl FnOnce() -> T) -> T {
    // Created first so guards and owning Arcs drop before registry cleanup,
    // including when an operation unwinds.
    let _cleanup = Cleanup;
    let mut keys: Vec<_> = paths.iter().map(|path| key(path)).collect();
    keys.sort_unstable();
    keys.dedup();
    let locks: Vec<_> = {
        let mut registry = LOCKS
            .get_or_init(Mutex::default)
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // No permanently retained entry per file ever touched by a session.
        registry.retain(|_, value| value.strong_count() != 0);
        keys.into_iter()
            .map(|key| {
                if let Some(lock) = registry.get(&key).and_then(Weak::upgrade) {
                    return lock;
                }
                let lock = Arc::new(Mutex::new(()));
                registry.insert(key, Arc::downgrade(&lock));
                lock
            })
            .collect()
    };
    let guards: Vec<_> = locks
        .iter()
        .map(|lock| lock.lock().unwrap_or_else(|e| e.into_inner()))
        .collect();
    let result = action();
    drop(guards);
    result
}

fn key(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    };
    let mut cleaned = PathBuf::new();
    for part in absolute.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                cleaned.pop();
            }
            value => cleaned.push(value.as_os_str()),
        }
    }
    if let Ok(target) = cleaned.canonicalize() {
        return target;
    }
    // Resolve the nearest existing ancestor so the key stays stable while
    // Write creates missing directories through an aliased parent.
    let mut missing = Vec::new();
    let mut ancestor = cleaned.as_path();
    while let (Some(parent), Some(name)) = (ancestor.parent(), ancestor.file_name()) {
        missing.push(name);
        if let Ok(mut resolved) = parent.canonicalize() {
            for part in missing.iter().rev() {
                resolved.push(part);
            }
            return resolved;
        }
        ancestor = parent;
    }
    cleaned
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::symlink};

    #[test]
    fn aliases_missing_ancestors_and_unwind_release_registry_entries() {
        let root = tempfile::tempdir().unwrap();
        let real = root.path().join("real");
        fs::create_dir(&real).unwrap();
        let alias = root.path().join("alias");
        symlink(&real, &alias).unwrap();
        let future = alias.join("missing/deep/file");
        let expected = real.canonicalize().unwrap().join("missing/deep/file");
        assert_eq!(key(&future), expected);
        with_paths(&[&future, &expected], || {
            fs::create_dir_all(future.parent().unwrap()).unwrap();
            fs::write(&future, "one").unwrap();
        });
        assert_eq!(key(&future), expected);
        let failure = std::panic::catch_unwind(|| with_paths(&[&future], || panic!("fixture")));
        assert!(failure.is_err());
        with_paths(&[&future], || {});
        assert!(LOCKS.get().unwrap().lock().unwrap().is_empty());
    }
}
