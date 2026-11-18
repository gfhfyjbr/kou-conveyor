//! Shell integration: telling the terminal where a shell's prompt is.
//!
//! A shell draws its prompt again whenever its terminal changes size, and a
//! terminal that re-wraps its lines on a resize keeps the old copies: after a
//! split, a closed pane or a dragged divider, what is left of earlier prompts
//! runs together beside the new one. Terminals deal with it one way: the
//! shell marks its prompt (OSC 133), and the terminal clears a marked prompt
//! before it resizes, because the shell is about to draw it again. Ghostty's
//! library does the clearing, for a prompt whose mark says the shell redraws
//! it (`redraw=1`). This module makes zsh send the marks, without
//! touching the user's own files: zsh is pointed at a directory of the app's
//! through `ZDOTDIR`, and the one file there puts `ZDOTDIR` back, reads the
//! user's `.zshenv`, and adds two hooks.
//!
//! Other shells are started as they are; their prompts are not marked.

use std::path::{Path, PathBuf};

/// The variable the user's own `ZDOTDIR` rides in while zsh starts.
const KEPT: &str = "KOU_ZDOTDIR";

/// What zsh reads instead of the user's `.zshenv`, and before the rest of
/// their files, which it reads from where they always are.
const ZSHENV: &str = r#"# kou desktop's shell integration for zsh.
#
# It marks where each prompt is (OSC 133), so that the terminal can clear a
# prompt zsh is about to draw again after a resize, instead of keeping what is
# left of the old one. zsh read this file because ZDOTDIR pointed here; that is
# put back first, so everything of yours is read from where it always is.
if [[ -n ${KOU_ZDOTDIR+set} ]]; then
    ZDOTDIR=$KOU_ZDOTDIR
    builtin unset KOU_ZDOTDIR
else
    builtin unset ZDOTDIR
fi

# Your own .zshenv, which zsh would have read instead of this file.
if [[ -r ${ZDOTDIR:-$HOME}/.zshenv ]]; then
    builtin source ${ZDOTDIR:-$HOME}/.zshenv
fi

if [[ -o interactive ]]; then
    builtin typeset -g _kou_ran=

    # Before each prompt: say how the last command ended, and put the marks
    # into the prompt itself, so that every redraw of the prompt sends them.
    _kou_marks_precmd() {
        builtin local last=$?
        builtin emulate -L zsh
        if [[ -n $_kou_ran ]]; then
            builtin print -rn -- $'\e]133;D;'$last$'\a'
            _kou_ran=
        fi
        # "redraw=1" says that zsh draws its prompt again after a resize,
        # which is what lets the terminal clear the old one first.
        if [[ $PS1 != *$'\e]133;A'* ]]; then
            PS1=$'%{\e]133;A;redraw=1\a%}'$PS1$'%{\e]133;B\a%}'
        fi
        # Stay last: a theme that rebuilds the prompt in a hook of its own
        # has done so by the time this runs.
        if [[ ${precmd_functions[-1]} != _kou_marks_precmd ]]; then
            precmd_functions=(${precmd_functions:#_kou_marks_precmd} _kou_marks_precmd)
        fi
    }

    # Before each command: what follows is its output, not the prompt.
    _kou_marks_preexec() {
        builtin print -rn -- $'\e]133;C\a'
        _kou_ran=1
    }

    precmd_functions+=(_kou_marks_precmd)
    preexec_functions+=(_kou_marks_preexec)
fi
"#;

/// Whether a program is zsh, by its name.
fn is_zsh(program: &Path) -> bool {
    program
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.trim_start_matches('-') == "zsh")
}

/// The variables to start a shell with so that it marks its prompts, or none
/// when the program is not a shell this knows. `home` is where the app keeps
/// its files; `zdotdir` is the `ZDOTDIR` the shell would otherwise have had.
pub(crate) fn environment(
    program: &Path,
    home: &Path,
    zdotdir: Option<String>,
) -> std::io::Result<Vec<(String, String)>> {
    if !is_zsh(program) {
        return Ok(Vec::new());
    }
    let directory: PathBuf = home.join("shell").join("zsh");
    let file = directory.join(".zshenv");
    // Written again only when it differs: shells already running read it once.
    if std::fs::read_to_string(&file).ok().as_deref() != Some(ZSHENV) {
        std::fs::create_dir_all(&directory)?;
        std::fs::write(&file, ZSHENV)?;
    }
    let mut variables = vec![(
        "ZDOTDIR".to_owned(),
        directory.to_string_lossy().into_owned(),
    )];
    if let Some(kept) = zdotdir {
        variables.push((KEPT.to_owned(), kept));
    }
    Ok(variables)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zsh_is_pointed_at_the_apps_file_and_keeps_its_own_directory() {
        let home = tempfile::tempdir().unwrap();
        let variables = environment(
            Path::new("/bin/zsh"),
            home.path(),
            Some("/Users/me/.zsh".into()),
        )
        .unwrap();
        let directory = home.path().join("shell/zsh");
        assert_eq!(
            variables,
            [
                (
                    "ZDOTDIR".to_owned(),
                    directory.to_string_lossy().into_owned()
                ),
                ("KOU_ZDOTDIR".to_owned(), "/Users/me/.zsh".to_owned()),
            ]
        );
        let written = std::fs::read_to_string(directory.join(".zshenv")).unwrap();
        assert!(written.contains("133;A") && written.contains("133;C"));
        // Without a directory of its own there is nothing to keep, and a login
        // shell is still zsh.
        let plain = environment(Path::new("-zsh"), home.path(), None).unwrap();
        assert_eq!(plain.len(), 1);
        // Other programs are left alone.
        for other in [
            "/bin/bash",
            "/opt/homebrew/bin/fish",
            "claude",
            "zsh-static",
        ] {
            assert!(
                environment(Path::new(other), home.path(), None)
                    .unwrap()
                    .is_empty()
            );
        }
    }
}
