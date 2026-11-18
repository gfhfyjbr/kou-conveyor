//! Finder and the Dock start bundles through launchd, outside the user's shell.

use nix::{
    sys::signal::{Signal, killpg},
    unistd::{Pid, Uid, User, getppid},
};
use std::{
    collections::HashMap,
    env,
    ffi::{OsStr, OsString},
    io::{self, IsTerminal as _, Read as _},
    os::{
        fd::OwnedFd,
        unix::{ffi::OsStrExt as _, net::UnixStream, process::CommandExt as _},
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

pub(crate) fn inside_a_bundle(program: &Path) -> bool {
    program.ancestors().any(|dir| {
        dir.extension() == Some(OsStr::new("app"))
            && program.starts_with(dir.join("Contents/MacOS"))
    })
}

pub(crate) fn prepare() -> io::Result<()> {
    const READY: &str = "KOU_FINDER_ENV_PID";
    let pid = std::process::id().to_string();
    if getppid().as_raw() != 1 || io::stdin().is_terminal() || env::var(READY).as_ref() == Ok(&pid)
    {
        return Ok(());
    }
    let program = env::current_exe()?;
    if !inside_a_bundle(&program) {
        return Ok(());
    }
    let Some(home) = dirs::home_dir() else {
        return Ok(());
    };
    let shell = env::var_os("SHELL")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute() && path.is_file())
        .or_else(|| {
            User::from_uid(Uid::current())
                .ok()
                .flatten()
                .map(|user| user.shell)
        })
        .unwrap_or_else(|| PathBuf::from("/bin/zsh"));
    let mut login = Command::new(&shell);
    login.current_dir(&home).env("SHELL", &shell);
    let mut vars = match login_environment(login, Duration::from_secs(3)) {
        Ok(vars) => vars,
        Err(error) => {
            eprintln!("kou-desktop: login shell environment unavailable: {error}");
            HashMap::new()
        }
    };
    let path = vars
        .remove(OsStr::new("PATH"))
        .or_else(|| env::var_os("PATH"));
    let mut app = Command::new(program);
    for (name, value) in vars {
        // The shell's bookkeeping and a parent pane's identity belong to their own processes.
        if !matches!(
            name.as_bytes(),
            b"PWD"
                | b"OLDPWD"
                | b"SHLVL"
                | b"_"
                | b"KOU_PANE"
                | b"KOU_SESSION"
                | b"KOU_SOCKET"
                | b"KOU_BIN"
        ) && env::var_os(&name).is_none()
        {
            app.env(name, value);
        }
    }
    if env::current_dir().ok().as_deref() == Some(Path::new("/")) {
        app.current_dir(&home);
    }
    // exec preserves the PID LaunchServices tracks and installs the environment before any GUI workers start.
    Err(app
        .args(env::args_os().skip(1))
        .env("PATH", search_path(path.as_deref(), &home)?)
        .env("SHELL", shell)
        .env(READY, pid)
        .exec())
}

fn search_path(path: Option<&OsStr>, home: &Path) -> io::Result<OsString> {
    let system = OsStr::new("/usr/bin:/bin:/usr/sbin:/sbin");
    let mut dirs: Vec<_> =
        env::split_paths(path.filter(|p| !p.is_empty()).unwrap_or(system)).collect();
    for dir in [
        home.join(".local/bin"),
        "/opt/homebrew/bin".into(),
        "/usr/local/bin".into(),
    ] {
        if dir.is_dir() && !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    env::join_paths(dirs).map_err(io::Error::other)
}

const BEGIN: &[u8] = b"\0KOU_LOGIN_ENV\0";
// Older macOS releases lack env -0. Frame the exports in one write stream,
// including values with newlines and a final empty record for backgrounded shells.
const DUMP: &str = r#"exec /usr/bin/awk 'BEGIN { printf "%cKOU_LOGIN_ENV%c", 0, 0; for (k in ENVIRON) printf "%s=%s%c", k, ENVIRON[k], 0; printf "%c", 0; exit }'"#;

fn login_environment(
    mut shell: Command,
    timeout: Duration,
) -> io::Result<HashMap<OsString, OsString>> {
    let (mut input, output) = UnixStream::pair()?;
    match Path::new(shell.get_program())
        .file_name()
        .and_then(OsStr::to_str)
    {
        Some("csh" | "tcsh") => {
            shell.arg0("-").args(["-c", DUMP]);
        }
        Some("nu") => {
            shell.args(["-l", "-e", DUMP]);
        }
        _ => {
            shell.args(["-l", "-i", "-c", DUMP]);
        }
    }
    let mut child = shell
        .stdin(Stdio::null())
        .stdout(Stdio::from(OwnedFd::from(output)))
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()?;
    drop(shell);
    let result = read_environment(&mut input, Instant::now() + timeout);
    if result.is_err() {
        let _ = killpg(Pid::from_raw(child.id() as i32), Signal::SIGKILL);
    }
    // The last delimiter can arrive before a logout script exits, or while a background job still holds stdout.
    let _ = child.kill();
    let _ = child.wait();
    result
}

fn read_environment(
    input: &mut UnixStream,
    deadline: Instant,
) -> io::Result<HashMap<OsString, OsString>> {
    let mut bytes = Vec::new();
    let mut chunk = [0; 8192];
    while bytes.len() < 1024 * 1024 {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(io::ErrorKind::TimedOut.into());
        }
        input.set_read_timeout(Some(left))?;
        let count = input.read(&mut chunk)?;
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..count]);
        let Some(start) = bytes.windows(BEGIN.len()).position(|part| part == BEGIN) else {
            continue;
        };
        let payload = &bytes[start + BEGIN.len()..];
        let Some(end) = payload.windows(2).position(|part| part == b"\0\0") else {
            continue;
        };
        let mut vars = HashMap::new();
        for pair in payload[..end].split(|&byte| byte == 0) {
            if let Some(at) = pair.iter().position(|&byte| byte == b'=')
                && at > 0
            {
                vars.insert(
                    OsStr::from_bytes(&pair[..at]).to_owned(),
                    OsStr::from_bytes(&pair[at + 1..]).to_owned(),
                );
            }
        }
        return Ok(vars);
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        "incomplete shell environment",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn reads_login_and_interactive_exports_despite_a_noisy_profile() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("tools with spaces");
        fs::create_dir(&bin).unwrap();
        fs::write(
            dir.path().join(".zprofile"),
            "export KOU_PROFILE='one=two'\n",
        )
        .unwrap();
        fs::write(
            dir.path().join(".zshrc"),
            "print -r -- 'login banner'\nprint -ru2 -- 'shell notice'\nexport PATH=\"$HOME/tools with spaces:$PATH\"\nexport KOU_MULTILINE=$'first\\nsecond'\n",
        ).unwrap();
        let mut shell = Command::new("/bin/zsh");
        shell
            .env_clear()
            .env("HOME", dir.path())
            .env("ZDOTDIR", dir.path())
            .env("PATH", "/usr/bin:/bin");
        let env = login_environment(shell, Duration::from_secs(3)).unwrap();
        assert_eq!(env.get(OsStr::new("KOU_PROFILE")).unwrap(), "one=two");
        assert_eq!(
            env.get(OsStr::new("KOU_MULTILINE")).unwrap(),
            "first\nsecond"
        );
        assert_eq!(env::split_paths(&env[OsStr::new("PATH")]).next(), Some(bin));
    }

    #[test]
    fn a_stalled_profile_has_a_deadline() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(".zshenv"), "exec /bin/sleep 20\n").unwrap();
        let mut shell = Command::new("/bin/zsh");
        shell
            .env_clear()
            .env("HOME", dir.path())
            .env("ZDOTDIR", dir.path());
        let started = Instant::now();
        assert!(login_environment(shell, Duration::from_millis(100)).is_err());
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn fallback_finds_the_local_bin_without_displacing_the_shells_choice() {
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join(".local/bin");
        fs::create_dir_all(&local).unwrap();
        let before = env::join_paths([Path::new("/custom tools"), &local]).unwrap();
        let after = search_path(Some(&before), dir.path()).unwrap();
        let paths: Vec<_> = env::split_paths(&after).collect();
        assert_eq!(paths[0], Path::new("/custom tools"));
        assert_eq!(paths.iter().filter(|path| **path == local).count(), 1);
        assert!(
            env::split_paths(&search_path(None, dir.path()).unwrap()).any(|path| path == local)
        );
    }
}
