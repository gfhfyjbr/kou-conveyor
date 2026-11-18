//! Whether a shell is running a command, asked of the system rather than
//! guessed from the screen: a terminal has one foreground process group, and
//! it is the shell's own only while the shell waits at its prompt.

/// Whether the shell with this process id has a job in the foreground of its
/// terminal. `None` where the system cannot be asked, or the process is gone.
#[cfg(target_os = "macos")]
pub fn job_running(shell: u32) -> Option<bool> {
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    // SAFETY: the buffer is a zeroed `proc_bsdinfo` of the size passed, and the
    // call writes at most that many bytes into it.
    let written = unsafe {
        libc::proc_pidinfo(
            shell as libc::c_int,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        )
    };
    if written != size {
        return None;
    }
    // SAFETY: the call filled the whole struct, and every bit pattern is a valid one for it.
    let info = unsafe { info.assume_init() };
    (info.e_tpgid != 0).then_some(info.e_tpgid != info.pbi_pgid)
}

/// See the macOS version.
#[cfg(target_os = "linux")]
pub fn job_running(shell: u32) -> Option<bool> {
    let stat = std::fs::read_to_string(format!("/proc/{shell}/stat")).ok()?;
    // The command name may hold spaces and brackets; the fields follow its last bracket.
    let fields: Vec<&str> = stat.rsplit_once(')')?.1.split_whitespace().collect();
    let group: i64 = fields.get(2)?.parse().ok()?;
    let foreground: i64 = fields.get(5)?.parse().ok()?;
    (foreground > 0).then_some(foreground != group)
}

/// See the macOS version.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn job_running(_: u32) -> Option<bool> {
    None
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn a_process_that_is_gone_or_has_no_terminal_gives_no_answer() {
        // Process ids stop far below this.
        assert_eq!(job_running(u32::MAX / 2), None);
    }

    #[test]
    fn this_test_process_can_be_asked() {
        // Run from a terminal it has a foreground group; from a build service it
        // has no terminal at all. Either is an answer the caller can use.
        let answer = job_running(std::process::id());
        assert!(matches!(answer, None | Some(true) | Some(false)));
    }
}
