# kou-conveyor terminal: the prompt and the integration, for fish.
#
# The terminal starts fish as a login shell with --init-command, which runs
# this after the user's config.fish; the user's files are never changed. It
# sets the kou-conveyor prompt and tells the terminal where the shell is
# (OSC 7) and its title:
#
#   ▪ …/Go/kou-conveyor/cmd main* ❯                                4.2s ✕ 1
#
# Turning the theme off in the cockpit's settings starts fish without it.

function fish_prompt
    set -l code $status
    set -l arrow (set_color 208)❯(set_color normal)
    test $code -ne 0; and set arrow (set_color red)❯(set_color normal)
    set -l path (prompt_pwd --full-length-dirs 1 2>/dev/null; or prompt_pwd)
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
