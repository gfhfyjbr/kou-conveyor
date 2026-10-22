# kou-conveyor terminal: the prompt and the integration, for fish.
#
# The terminal starts fish as a login shell with --init-command, which runs
# this after the user's config.fish; the user's files are never changed. It
# sets the kou-conveyor prompt and tells the terminal where the shell is
# (OSC 7), its title, and where each prompt, command and its output start
# (OSC 133):
#
#   ▪ …/Go/kou-conveyor/cmd main* ❯                                4.2s ✕ 1
#
# Turning the theme off in the cockpit's settings starts fish without it.

# Where the prompt starts (OSC 133 A). fish 4.1 and later move the cursor to
# a click in the command line that the terminal tells them of (click_events),
# as in Ghostty.
set -g __kou_prompt_mark \e']133;A'\a
set -l __kou_version (string match -r '^(\d+)\.(\d+)' -- $version)
if test "$__kou_version[2]" -gt 4 2>/dev/null; or test "$__kou_version[2]" -eq 4 -a "$__kou_version[3]" -ge 1 2>/dev/null
    set __kou_prompt_mark \e']133;A;click_events=1'\a
end

function fish_prompt
    set -l code $status
    printf '%s' $__kou_prompt_mark
    set -l arrow (set_color 208)❯(set_color normal)
    test $code -ne 0; and set arrow (set_color red)❯(set_color normal)
    # fish 4's prompt_pwd can print the path and fail all the same: what it
    # printed is the path, when it printed one.
    set -l path (prompt_pwd --full-length-dirs 1 2>/dev/null)
    test -n "$path"; or set path (prompt_pwd)
    set -l vcs
    set -l branch (command git symbolic-ref --short HEAD 2>/dev/null; or command git rev-parse --short HEAD 2>/dev/null)
    if test -n "$branch"
        if test -n "$(command git status --porcelain --ignore-submodules=dirty -uno 2>/dev/null | head -n1)"
            set vcs ' '(set_color yellow)$branch'*'(set_color normal)
        else
            set vcs ' '(set_color green)$branch(set_color normal)
        end
    end
    printf '%s▪%s %s%s%s%s %s ' (set_color 208) (set_color normal) (set_color --bold) $path (set_color normal) "$vcs" $arrow
    printf '\e]133;B\a'
end

# At the prompt the cursor is a blinking bar, as Ghostty has it, unless fish's
# vi mode shapes it.
function __kou_cursor --on-event fish_prompt
    functions -q fish_vi_cursor_handle; or printf '\e[5 q'
end

# Where a command's output starts, after the cursor the terminal has of its
# own is back, and where the command ended with its code (OSC 133 C and D).
function __kou_preexec --on-event fish_preexec
    functions -q fish_vi_cursor_handle; or printf '\e[0 q'
    printf '\e]133;C\a'
end

function __kou_postexec --on-event fish_postexec
    printf '\e]133;D;%s\a' $status
end

# A terminal of a canvas finds its kou-canvas first.
if set -q KOU_CANVAS_BIN; and test -n "$KOU_CANVAS_BIN"
    set -gx PATH $KOU_CANVAS_BIN $PATH
end

function fish_right_prompt
    set -l code $status
    set -l took
    if test -n "$CMD_DURATION"; and test $CMD_DURATION -ge 2000
        set took (set_color brblack)(math --scale 1 $CMD_DURATION / 1000)s(set_color normal)
    end
    set -l failed
    test $code -ne 0; and set failed ' '(set_color red)'✕ '$code(set_color normal)
    printf '%s%s' "$took" "$failed"
end

function fish_title
    if set -q argv[1]
        echo -- $argv[1]
    else
        prompt_pwd
    end
end

function __kou_report_dir --on-variable PWD
    printf '\e]7;file://%s%s\a' $hostname (string escape --style=url -- $PWD)
end
__kou_report_dir
