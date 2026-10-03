# kou-conveyor terminal: the prompt and the integration, for zsh.
#
# Sourced after the user's .zshrc (see .zshenv), it takes the prompt over
# from whatever that set up — Powerlevel10k, Starship, Pure, an Oh My Zsh
# theme — and tells the terminal where the shell is (OSC 7), its title
# (OSC 2) and where each prompt, command and its output start (OSC 133): new
# splits start where the shell is, and the sidebar's tabs show the title.
#
#   ▪ …/Go/kou-conveyor/cmd main* ↑1 ❯                          4.2s ✕ 1
#
# The mark and the arrow are the cockpit's accent — colour 208, which the
# terminal's theme makes it — and the arrow turns red when a command fails.
# The branch is green, yellow with a * while the work tree has changes;
# git is asked about them in the background, so the prompt never waits.

[[ -o interactive ]] || return 0
zmodload zsh/datetime 2>/dev/null
autoload -Uz add-zsh-hook

# ---------------------------------------------------------------- taking over

() {
  emulate -L zsh
  # Powerlevel10k and Powerlevel9k take themselves away.
  (( $+functions[prompt_powerlevel10k_teardown] )) && prompt_powerlevel10k_teardown 2>/dev/null
  (( $+functions[prompt_powerlevel9k_teardown] )) && prompt_powerlevel9k_teardown 2>/dev/null
  # The hooks prompt frameworks draw their prompts with.
  local hook fn
  local -a keep
  for hook in precmd preexec chpwd; do
    keep=()
    for fn in ${(P)${:-${hook}_functions}}; do
      case $fn in
        (starship_*|prompt_starship_*|_p9k_*|powerlevel9k_*|prompt_pure_*|prompt_spaceship_*|spaceship_*|_omp_*|_geometry_*|prompt_*_precmd|prompt_*_preexec) ;;
        (*) keep+=($fn) ;;
      esac
    done
    set -A ${hook}_functions "${keep[@]}"
  done
}

setopt prompt_subst prompt_percent transient_rprompt

typeset -g _kou_branch= _kou_repo= _kou_dirty= _kou_ahead=0 _kou_behind=0
typeset -g _kou_vcs= _kou_took= _kou_started= _kou_ran=
typeset -gi _kou_git_fd=0

# ---------------------------------------------------------------- telling the terminal

# _kou_title sets the title the sidebar's tab shows.
_kou_title() {
  builtin print -rn -- $'\e]2;'"${1//[[:cntrl:]]/}"$'\a'
}

# _kou_report_dir tells the terminal where the shell is, as a file URL.
_kou_report_dir() {
  emulate -L zsh
  setopt no_multibyte
  local s=$PWD c hex out=
  local -i i
  for (( i = 1; i <= $#s; i++ )); do
    c=$s[i]
    if [[ $c == [a-zA-Z0-9/._~-] ]]; then
      out+=$c
    else
      builtin printf -v hex '%%%02X' "'$c"
      out+=$hex
    fi
  done
  builtin print -rn -- $'\e]7;file://'"${HOST}${out}"$'\a'
}

# ---------------------------------------------------------------- git

# _kou_git_head finds the branch from the repository's HEAD, without git.
_kou_git_head() {
  emulate -L zsh
  _kou_branch= _kou_repo=
  local dir=$PWD git head line
  while :; do
    git=$dir/.git
    if [[ -d $git ]]; then
      head=$git/HEAD
      break
    elif [[ -f $git ]]; then
      { read -r line < $git } 2>/dev/null
      line=${line#gitdir: }
      [[ $line == /* ]] || line=$dir/$line
      head=$line/HEAD
      break
    fi
    [[ $dir == / || -z $dir ]] && return
    dir=${dir:h}
  done
  { read -r line < $head } 2>/dev/null || return
  _kou_repo=$dir
  case $line in
    ('ref: refs/heads/'*) _kou_branch=${line#ref: refs/heads/} ;;
    ('ref: '*) _kou_branch=${line#ref: } ;;
    (*) _kou_branch=${line[1,7]} ;;
  esac
}

# _kou_vcs_update writes the branch part of the prompt.
_kou_vcs_update() {
  emulate -L zsh
  if [[ -z $_kou_branch ]]; then
    _kou_vcs=
    return
  fi
  local color=2 mark=
  [[ -n $_kou_dirty ]] && { color=3; mark='*' }
  _kou_vcs=" %F{$color}${_kou_branch//\%/%%}${mark}%f"
  (( _kou_ahead > 0 )) && _kou_vcs+=" %F{8}↑${_kou_ahead}%f"
  (( _kou_behind > 0 )) && _kou_vcs+=" %F{8}↓${_kou_behind}%f"
}

# _kou_git_status asks git, in the background, whether the work tree has
# changes and how far the branch is from its upstream.
_kou_git_status() {
  emulate -L zsh
  if (( _kou_git_fd )); then
    zle -F $_kou_git_fd 2>/dev/null
    exec {_kou_git_fd}<&-
    _kou_git_fd=0
  fi
  if [[ -z $_kou_repo ]]; then
    _kou_dirty= _kou_ahead=0 _kou_behind=0
    return
  fi
  exec {_kou_git_fd}< <(
    GIT_OPTIONAL_LOCKS=0 command git -C $_kou_repo status --porcelain=v2 --branch --ignore-submodules=dirty 2>/dev/null |
      command awk '/^# branch\.ab / { a = substr($3, 2) + 0; b = substr($4, 2) + 0; next } /^#/ { next } { d = 1 } END { printf "%d %d %d\n", d, a, b }'
  )
  zle -F -w $_kou_git_fd _kou_git_done 2>/dev/null
}

# _kou_git_done takes git's answer, and draws the prompt again if it
# changed anything.
_kou_git_done() {
  emulate -L zsh
  local -i fd=$1
  local line
  zle -F $fd 2>/dev/null
  read -r -u $fd line
  exec {fd}<&-
  (( fd == _kou_git_fd )) && _kou_git_fd=0
  local -a parts=(${=line})
  local dirty=
  (( ${parts[1]:-0} )) && dirty=1
  local ahead=${parts[2]:-0} behind=${parts[3]:-0}
  if [[ $dirty != $_kou_dirty || $ahead != $_kou_ahead || $behind != $_kou_behind ]]; then
    _kou_dirty=$dirty _kou_ahead=$ahead _kou_behind=$behind
    _kou_vcs_update
    zle .reset-prompt 2>/dev/null
  fi
}
zle -N _kou_git_done

# ---------------------------------------------------------------- the prompt

# _kou_duration says how long a command took: 4.2s, 1m 03s, 1h 02m.
_kou_duration() {
  emulate -L zsh
  local -F seconds=$1
  local -i whole=$seconds
  if (( seconds < 60 )); then
    builtin printf -v REPLY '%.1fs' $seconds
  elif (( whole < 3600 )); then
    builtin printf -v REPLY '%dm %02ds' $(( whole / 60 )) $(( whole % 60 ))
  else
    builtin printf -v REPLY '%dh %02dm' $(( whole / 3600 )) $(( whole / 60 % 60 ))
  fi
}

typeset -g _kou_prompt=$'%{\e]133;A\a%}%F{208}▪%f %B%(4~|…/%3~|%~)%b${_kou_vcs} %(?.%F{208}.%F{1})❯%f %{\e]133;B\a%}'
typeset -g _kou_rprompt='${_kou_took}%(?.. %F{1}✕ %?%f)'

_kou_preexec() {
  _kou_started=$EPOCHREALTIME
  _kou_ran=1
  builtin print -n $'\e]133;C\a'
  _kou_title "${1[1,120]}"
}

_kou_precmd() {
  local -i code=$?
  emulate -L zsh
  if [[ -n $_kou_ran ]]; then
    builtin print -n $'\e]133;D;'"$code"$'\a'
    _kou_ran=
  fi
  _kou_took=
  if [[ -n $_kou_started ]]; then
    local -F took=$(( EPOCHREALTIME - _kou_started ))
    _kou_started=
    if (( took >= 2 )); then
      _kou_duration $took
      _kou_took="%F{8}${REPLY}%f"
    fi
  fi
  _kou_report_dir
  _kou_title "${(%):-%~}"
  _kou_git_head
  _kou_vcs_update
  _kou_git_status
  # Last of the hooks, the prompt is kou-conveyor's whatever ran before.
  PROMPT=$_kou_prompt RPROMPT=$_kou_rprompt PS2='%F{8}…%f '
}

add-zsh-hook preexec _kou_preexec
add-zsh-hook precmd _kou_precmd
PROMPT=$_kou_prompt RPROMPT=$_kou_rprompt PS2='%F{8}…%f '

# A terminal of a canvas finds its kou-canvas first.
if [[ -n ${KOU_CANVAS_BIN-} ]]; then
  path=($KOU_CANVAS_BIN $path)
fi
