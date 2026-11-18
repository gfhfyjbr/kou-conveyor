//! Builds libghostty-vt from the release tarballs in `vendor/` with Zig and links it
//! statically.
//!
//! Nothing is downloaded: the source tarball holds the library and the dependency
//! tarball every Zig package its build needs, so Zig runs with `--system` and an empty
//! package cache. The result is cached in `OUT_DIR` and rebuilt only when the tarballs,
//! this script or `ZIG` change.

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

/// The libghostty-vt release.
const VERSION: &str = "1.0.0";
/// The Ghostty commit the release was cut from: tag `vt-1.0.0`.
const COMMIT: &str = "0fa3d577a5614150ff8e2c152de5fb9d5c6d2dec";
/// The two tarballs from `release.files.ghostty.org/libghostty-vt/1.0.0/` and the
/// checksums the release's `SHA256SUMS` lists for them.
const TARBALLS: [(&str, &str); 2] = [
    (
        "libghostty-vt-1.0.0.tar.gz",
        "4a6ef758be721923e1969e7b81b443dcc92d8dd3fb5a39600c03a8817a753676",
    ),
    (
        "libghostty-vt-1.0.0-deps.tar.gz",
        "4d5ea5e448e4c7ea8fb55ccbaa8668f2a97d44f438cee831e2b7ecd162d85c29",
    ),
];
/// The release requires Zig 0.17: any 0.17.x patch release, not 0.16 or 0.18.
const ZIG_LINE: &str = "0.17.";
/// Where the Zig release to use can be unpacked, relative to this crate.
const ZIG_VENDOR: &str = "../../vendor";

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("set by cargo"));
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("set by cargo"));
    let vendor = manifest.join("vendor");
    println!("cargo:rerun-if-changed=build.rs");
    for (name, _) in TARBALLS {
        println!("cargo:rerun-if-changed={}", vendor.join(name).display());
    }
    println!("cargo:rerun-if-env-changed=ZIG");

    let install = out.join("install");
    let library = install.join("lib").join("libghostty-vt.a");
    let stamp = out.join("built-from");
    let wanted = format!("libghostty-vt {VERSION} ({COMMIT}) for {}", zig_target());
    let fresh = library.is_file() && fs::read_to_string(&stamp).ok().as_deref() == Some(&wanted);
    if !fresh {
        build(&manifest, &vendor, &out, &install);
        fs::write(&stamp, &wanted).expect("record the build");
    }

    println!(
        "cargo:rustc-link-search=native={}",
        install.join("lib").display()
    );
    println!("cargo:rustc-link-lib=static=ghostty-vt");
    // The library's SIMD paths are C++ (simdutf, highway).
    if env::var("CARGO_CFG_TARGET_VENDOR").as_deref() == Ok("apple") {
        println!("cargo:rustc-link-lib=c++");
    } else {
        println!("cargo:rustc-link-lib=stdc++");
    }
    println!("cargo:include={}", install.join("include").display());
}

fn build(manifest: &Path, vendor: &Path, out: &Path, install: &Path) {
    let zig = find_zig(manifest);
    for (name, checksum) in TARBALLS {
        verify(&vendor.join(name), checksum);
    }

    let sources = out.join("src");
    let global_cache = out.join("zig-global-cache");
    let local_cache = out.join("zig-local-cache");
    for directory in [
        &sources,
        &global_cache,
        &local_cache,
        &install.to_path_buf(),
    ] {
        let _ = fs::remove_dir_all(directory);
    }
    fs::create_dir_all(&sources).expect("create the source directory");
    // Zig 0.17 can crash on a package cache that already holds archives, and an empty
    // one proves the build used only the vendored packages.
    fs::create_dir_all(&global_cache).expect("create the Zig cache");
    for (name, _) in TARBALLS {
        run(Command::new("tar")
            .arg("-xzf")
            .arg(vendor.join(name))
            .arg("-C")
            .arg(&sources));
    }

    let source = sources.join(format!("libghostty-vt-{VERSION}"));
    let packages = sources.join(format!("libghostty-vt-{VERSION}-deps"));
    run(Command::new(&zig)
        .current_dir(&source)
        .arg("build")
        .arg("--prefix")
        .arg(install)
        .arg("--system")
        .arg(&packages)
        .arg("-Doptimize=ReleaseFast")
        .arg(format!("-Dtarget={}", zig_target()))
        .arg("-Dcpu=baseline")
        .env("ZIG_GLOBAL_CACHE_DIR", &global_cache)
        .env("ZIG_LOCAL_CACHE_DIR", &local_cache));

    // Only the static library and the headers are needed; the rest is a few hundred
    // megabytes in a build directory several agents share.
    for directory in [&sources, &global_cache, &local_cache] {
        let _ = fs::remove_dir_all(directory);
    }
    if let Ok(entries) = fs::read_dir(install.join("lib")) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            if name.to_string_lossy().contains(".dylib") || name.to_string_lossy().contains(".so") {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
}

/// Zig's name for the target cargo builds for.
fn zig_target() -> String {
    let target = env::var("TARGET").expect("set by cargo");
    let zig = match target.as_str() {
        "aarch64-apple-darwin" => "aarch64-macos",
        "x86_64-apple-darwin" => "x86_64-macos",
        "aarch64-unknown-linux-gnu" => "aarch64-linux-gnu",
        "x86_64-unknown-linux-gnu" => "x86_64-linux-gnu",
        "aarch64-unknown-linux-musl" => "aarch64-linux-musl",
        "x86_64-unknown-linux-musl" => "x86_64-linux-musl",
        other => panic!("ghostty-vt-sys does not know Zig's name for the target {other}"),
    };
    zig.to_string()
}

/// The Zig to build with: `ZIG` if set, else a 0.17 release unpacked into
/// `desktop/vendor/`, else `zig` on the path if it is 0.17.
fn find_zig(manifest: &Path) -> PathBuf {
    let advice = format!(
        "libghostty-vt {VERSION} is built with Zig {ZIG_LINE}x (0.16 and 0.18 do not work). \
         Download zig-<arch>-<os>-0.17.1 from https://ziglang.org/download/ and unpack it \
         into desktop/vendor/ (for example desktop/vendor/zig-aarch64-macos-0.17.1/zig), \
         or point the ZIG environment variable at a Zig 0.17 binary."
    );
    if let Some(zig) = env::var_os("ZIG") {
        let zig = PathBuf::from(zig);
        match zig_version(&zig) {
            Some(version) if version.starts_with(ZIG_LINE) => return zig,
            Some(version) => panic!("ZIG is {} with Zig {version}. {advice}", zig.display()),
            None => panic!("ZIG is {}, which does not run. {advice}", zig.display()),
        }
    }

    let vendored = manifest.join(ZIG_VENDOR);
    let mut candidates: Vec<PathBuf> = fs::read_dir(&vendored)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name.starts_with("zig-") && name.contains(&format!("-{ZIG_LINE}"))
        })
        .map(|entry| entry.path().join("zig"))
        .filter(|zig| zig.is_file())
        .collect();
    // The newest patch release sorts last.
    candidates.sort();
    if let Some(zig) = candidates
        .into_iter()
        .rev()
        .find(|zig| zig_version(zig).is_some_and(|version| version.starts_with(ZIG_LINE)))
    {
        return zig;
    }

    let found = zig_version(Path::new("zig"));
    match found {
        Some(version) if version.starts_with(ZIG_LINE) => PathBuf::from("zig"),
        Some(version) => panic!("the zig on the path is {version}. {advice}"),
        None => panic!("no Zig found. {advice}"),
    }
}

fn zig_version(zig: &Path) -> Option<String> {
    let output = Command::new(zig).arg("version").output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Checks a tarball against the release's published checksum.
fn verify(path: &Path, expected: &str) {
    let tools: [(&str, &[&str]); 2] = [("shasum", &["-a", "256"]), ("sha256sum", &[])];
    for (tool, arguments) in tools {
        let Ok(output) = Command::new(tool).args(arguments).arg(path).output() else {
            continue;
        };
        if !output.status.success() {
            continue;
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let actual = text.split_whitespace().next().unwrap_or_default();
        assert_eq!(
            actual,
            expected,
            "{} does not match the libghostty-vt {VERSION} release",
            path.display()
        );
        return;
    }
    println!(
        "cargo:warning=no shasum or sha256sum to check {} with",
        path.display()
    );
}

fn run(command: &mut Command) {
    let status = command
        .status()
        .unwrap_or_else(|error| panic!("could not run {command:?}: {error}"));
    assert!(status.success(), "{command:?} failed with {status}");
}
