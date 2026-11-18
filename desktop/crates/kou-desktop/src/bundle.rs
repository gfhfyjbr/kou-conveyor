//! On macOS the app runs from an application bundle of its own.
//!
//! The system knows a program by its bundle: that is whose name and icon a
//! notification carries, and what a click on one brings to the front. A
//! program started as a bare file has no bundle, and its notifications could
//! only be posted in the name of some other program. So when the app is
//! started as a bare file (which is how it is built and run from this
//! repository), it first puts itself into a bundle under its home
//! (`<home>/KouDesktop.app`, a copy of the program with an `Info.plist` and
//! its icon), makes the system aware of it, and carries on as the program
//! inside. Same process, same arguments, same terminal; nothing is installed
//! anywhere else.
//!
//! `script/package` puts a release build into the same bundle, from the same
//! files in `resources/`, to hand out.

use std::{
    ffi::OsString,
    io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

/// The bundle's identifier: what the system files the app's notification
/// settings under.
const IDENTIFIER: &str = "dev.kou-conveyor.desktop";
// Another identifier for an instance started to check something, so that
// what the system remembers about it is not remembered about the app.
const IDENTIFIER_FROM: &str = "KOU_BUNDLE_ID";

// Set for the program inside the bundle, so that it never does this twice.
const INSIDE: &str = "KOU_BUNDLED";
const PROGRAM: &str = "kou-desktop";
// What the bundle says of itself, with places for what differs.
const INFO: &str = include_str!("../resources/Info.plist");
// Keep the resources of the local app bundle in sync with `script/package`.
const RESOURCES: &[(&str, &[u8])] = &[
    (
        "Assets.car",
        include_bytes!("../resources/macos/Assets.car"),
    ),
    (
        "AppIcon.icns",
        include_bytes!("../resources/macos/AppIcon.icns"),
    ),
    (
        "onest-OFL.txt",
        include_bytes!("../../kou-ui/fonts/onest/LICENSE.txt"),
    ),
    (
        "martian-mono-OFL.txt",
        include_bytes!("../../kou-ui/fonts/martian-mono/LICENSE.txt"),
    ),
];
const REGISTER: &str = "/System/Library/Frameworks/CoreServices.framework/Versions/A/Frameworks/\
                        LaunchServices.framework/Versions/A/Support/lsregister";

/// Carry on as the program inside the app's bundle under `home`, making or
/// refreshing the bundle first. Does not return when that works: the
/// process becomes the program inside. Returns the reason when it does not,
/// and immediately when this already is that program.
pub fn enter(home: &Path, arguments: &[String]) -> io::Result<()> {
    use std::os::unix::process::CommandExt as _;
    let program = std::env::current_exe()?;
    if std::env::var_os(INSIDE).is_some() || inside_a_bundle(&program) {
        return Ok(());
    }
    let identifier = std::env::var(IDENTIFIER_FROM).unwrap_or_else(|_| IDENTIFIER.to_owned());
    let bundle = home.join("KouDesktop.app");
    // Signed as a whole, to run on this machine: the system gives a bundle
    // whose contents are not sealed no notifications.
    let (inside, made) = refresh(&bundle, &program, &identifier, &|draft| {
        let _ = Command::new("codesign")
            .args(["--force", "--sign", "-"])
            .arg(draft)
            .output();
    })?;
    // Told to the system, which otherwise learns of a bundle only when it is
    // opened from the Finder, and posts nothing for one it does not know.
    // Every time, because what the system knows can be thrown away under
    // the app; but only a new bundle is worth waiting most of a second for.
    let mut register = Command::new(REGISTER);
    register
        .arg("-f")
        .arg(&bundle)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if made {
        let _ = register.status();
    } else {
        let _ = register.spawn();
    }
    Err(Command::new(inside).args(arguments).env(INSIDE, "1").exec())
}

fn inside_a_bundle(program: &Path) -> bool {
    program.to_string_lossy().contains(".app/Contents/MacOS/")
}

// Make `bundle` hold this program. Returns the program inside it, and
// whether the bundle was put together anew. Files only: `seal` is given a
// new bundle before it is moved into place, and nothing outside `bundle`'s
// folder is told of it.
fn refresh(
    bundle: &Path,
    program: &Path,
    identifier: &str,
    seal: &dyn Fn(&Path),
) -> io::Result<(PathBuf, bool)> {
    let inside = bundle.join("Contents/MacOS").join(PROGRAM);
    let plist = bundle.join("Contents/Info.plist");
    let wanted = info_plist(identifier);
    // Which build the bundle holds is written down in it: the copy itself
    // cannot say, because signing changes it.
    let stamp = bundle.join("Contents/Resources/built-from");
    let built = std::fs::metadata(program).map(|built| {
        let modified = built
            .modified()
            .ok()
            .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |since| since.as_nanos());
        format!("{} {modified}", built.len())
    })?;
    let stale = !inside.is_file()
        || std::fs::read_to_string(&plist).ok().as_deref() != Some(wanted.as_str())
        || std::fs::read_to_string(&stamp).ok().as_deref() != Some(built.as_str());
    if !stale {
        return Ok((inside, false));
    }
    // Built beside its place and moved in whole: nothing ever finds half a
    // bundle, and a program still running from the old one keeps its file.
    let beside = |tag: &str| {
        let mut path = OsString::from(bundle.as_os_str());
        path.push(format!(".{tag}{}", std::process::id()));
        PathBuf::from(path)
    };
    let draft = beside("new.");
    let _ = std::fs::remove_dir_all(&draft);
    std::fs::create_dir_all(draft.join("Contents/MacOS"))?;
    std::fs::copy(program, draft.join("Contents/MacOS").join(PROGRAM))?;
    std::fs::write(draft.join("Contents/Info.plist"), wanted)?;
    std::fs::create_dir_all(draft.join("Contents/Resources"))?;
    for &(name, bytes) in RESOURCES {
        std::fs::write(draft.join("Contents/Resources").join(name), bytes)?;
    }
    std::fs::write(draft.join("Contents/Resources/built-from"), built)?;
    seal(&draft);
    let old = beside("old.");
    let had = std::fs::rename(bundle, &old).is_ok();
    std::fs::rename(&draft, bundle)?;
    if had {
        let _ = std::fs::remove_dir_all(&old);
    }
    Ok((inside, true))
}

fn info_plist(identifier: &str) -> String {
    INFO.replace("{identifier}", identifier)
        .replace("{program}", PROGRAM)
        .replace("{version}", env!("CARGO_PKG_VERSION"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_program_is_put_into_a_bundle_once_and_again_when_it_changed() {
        let home = tempfile::tempdir().unwrap();
        let program = home.path().join("built");
        let bundle = home.path().join("KouDesktop.app");
        let sealed = std::cell::Cell::new(0);
        // Sealing changes the copy, as signing does.
        let seal = |draft: &Path| {
            assert!(draft.join("Contents/Info.plist").is_file());
            let copy = draft.join("Contents/MacOS/kou-desktop");
            let mut signed = std::fs::read(&copy).unwrap();
            signed.extend_from_slice(b" (signed)");
            std::fs::write(&copy, signed).unwrap();
            sealed.set(sealed.get() + 1);
        };
        std::fs::write(&program, b"first build").unwrap();
        let (inside, made) = refresh(&bundle, &program, IDENTIFIER, &seal).unwrap();
        assert!(made);
        assert_eq!(inside, bundle.join("Contents/MacOS/kou-desktop"));
        assert_eq!(std::fs::read(&inside).unwrap(), b"first build (signed)");
        let plist = std::fs::read_to_string(bundle.join("Contents/Info.plist")).unwrap();
        assert!(plist.contains("<string>dev.kou-conveyor.desktop</string>"));
        assert!(plist.contains("<string>kou-desktop</string>"));
        assert!(!plist.contains('{'), "a place left unfilled: {plist}");
        // The icon the plist names is there, in both of its forms.
        assert!(plist.contains("<key>CFBundleIconName</key>\n    <string>AppIcon</string>"));
        for name in ["Assets.car", "AppIcon.icns"] {
            let held = std::fs::metadata(bundle.join("Contents/Resources").join(name)).unwrap();
            assert!(held.len() > 1024, "{name} is {} bytes", held.len());
        }
        assert!(inside_a_bundle(&inside) && !inside_a_bundle(&program));
        assert_eq!(sealed.get(), 1);

        // The same program again: the bundle is left as it is.
        let written = std::fs::metadata(&inside).unwrap().modified().unwrap();
        let (_, again) = refresh(&bundle, &program, IDENTIFIER, &seal).unwrap();
        assert!(!again);
        assert_eq!(
            std::fs::metadata(&inside).unwrap().modified().unwrap(),
            written
        );
        assert_eq!(sealed.get(), 1);
        // Another build: the bundle holds it, and nothing is left beside it.
        std::fs::write(&program, b"a second, longer build").unwrap();
        refresh(&bundle, &program, IDENTIFIER, &seal).unwrap();
        assert_eq!(
            std::fs::read(&inside).unwrap(),
            b"a second, longer build (signed)"
        );
        // Another identifier: made again, though the program is the same.
        refresh(&bundle, &program, "dev.kou-conveyor.check", &seal).unwrap();
        let plist = std::fs::read_to_string(bundle.join("Contents/Info.plist")).unwrap();
        assert!(plist.contains("<string>dev.kou-conveyor.check</string>"));
        assert_eq!(sealed.get(), 3);
        let left: Vec<_> = std::fs::read_dir(home.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            left.iter()
                .all(|name| name == "built" || name == "KouDesktop.app"),
            "{left:?}"
        );
    }
}
