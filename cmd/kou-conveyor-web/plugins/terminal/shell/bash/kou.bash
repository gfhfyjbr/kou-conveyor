# kou-conveyor terminal: the prompt and the integration, for bash.
#
# The terminal starts bash with --rcfile this file. It reads what bash
# would have read — the files a login shell reads — and the user's files
# are never changed; then it sets the kou-conveyor prompt, and tells the
# terminal where the shell is (OSC 7), its title (OSC 2) and where each
# prompt, command and its output start and where the command ended
# (OSC 133):
#
#   ▪ …/Go/kou-conveyor/cmd main ❯
#
# Turning the theme off in the cockpit's settings starts bash without it.

if [[ -n ${KOU_CONVEYOR_BASH_LOGIN-} ]]; then
  unset KOU_CONVEYOR_BASH_LOGIN
  [[ -r /etc/profile ]] && . /etc/profile
  for _kou_file in ~/.bash_profile ~/.bash_login ~/.profile; do
    if [[ -r $_kou_file ]]; then
      . "$_kou_file"
      break
    fi
  done
  unset _kou_file
elif [[ -r ~/.bashrc ]]; then
  . ~/.bashrc
fi

shopt -s promptvars
_kou_short= _kou_vcs= _kou_arrow= _kou_url=

# _kou_urlencode encodes a path for a file URL.
_kou_urlencode() {
  local LC_ALL=C s=$1 out= c i
  for (( i = 0; i < ${#s}; i++ )); do
    c=${s:i:1}
    case $c in
      [a-zA-Z0-9/._~-]) out+=$c ;;
      *) printf -v c '%%%02X' "'$c"; out+=$c ;;
    esac
  done
  _kou_url=$out
}

# _kou_branch reads the branch from the repository's HEAD, without git.
_kou_branch() {
  local dir=$PWD line head=
  _kou_vcs=
  while :; do
    if [[ -d $dir/.git ]]; then
      head=$dir/.git/HEAD
      break
    elif [[ -f $dir/.git ]]; then
      read -r line < "$dir/.git" || return
      line=${line#gitdir: }
      [[ $line == /* ]] || line=$dir/$line
      head=$line/HEAD
      break
    fi
    [[ -z $dir || $dir == / ]] && return
    dir=${dir%/*}
    [[ -z $dir ]] && dir=/
  done
  read -r line < "$head" 2>/dev/null || return
  case $line in
    'ref: refs/heads/'*) line=${line#ref: refs/heads/} ;;
    'ref: '*) line=${line#ref: } ;;
    *) line=${line:0:7} ;;
  esac
  _kou_vcs=$' \001\e[32m\002'"${line}"$'\001\e[39m\002'
}

_kou_prompt() {
  local code=$?
  # The command before this prompt ended (OSC 133 D), with its code; the
  # first prompt follows none.
  if [[ -n ${_kou_prompted-} ]]; then
    printf '\e]133;D;%s\a' "$code"
  fi
  _kou_prompted=1
  local path=${PWD/#$HOME/\~}
  local -a parts
  IFS=/ read -r -a parts <<< "$path"
  local n=${#parts[@]}
  if (( n > 4 )); then
    _kou_short="…/${parts[n-3]}/${parts[n-2]}/${parts[n-1]}"
  else
    _kou_short=$path
  fi
  if (( code )); then
    _kou_arrow=$'\001\e[31m\002❯\001\e[39m\002'
  else
    _kou_arrow=$'\001\e[38;5;208m\002❯\001\e[39m\002'
  fi
  _kou_branch
  _kou_urlencode "$PWD"
  printf '\e]7;file://%s%s\a\e]2;%s\a' "${HOSTNAME-}" "$_kou_url" "${path//[[:cntrl:]]/}"
  return $code
}

# Starship and others set PS1 from PROMPT_COMMAND: theirs go, the rest stay.
_kou_prompt_commands() {
  local cmd
  if [[ "$(declare -p PROMPT_COMMAND 2>/dev/null)" == "declare -a"* ]]; then
    local -a keep=()
    for cmd in "${PROMPT_COMMAND[@]}"; do
      [[ $cmd == *starship* || $cmd == *_omp_* || $cmd == *powerline* ]] || keep+=("$cmd")
    done
    PROMPT_COMMAND=(_kou_prompt "${keep[@]}")
  else
    cmd=${PROMPT_COMMAND-}
    [[ $cmd == *starship* || $cmd == *_omp_* || $cmd == *powerline* ]] && cmd=
    PROMPT_COMMAND="_kou_prompt${cmd:+; $cmd}"
  fi
}
_kou_prompt_commands
unset -f _kou_prompt_commands

PS1=$'\001\e]133;A\a\002\001\e[38;5;208m\002▪\001\e[39m\002 \001\e[1m\002${_kou_short}\001\e[22m\002${_kou_vcs} ${_kou_arrow} \001\e]133;B\a\002'
PS2=$'\001\e[90m\002…\001\e[39m\002 '
# Where a command's output starts (OSC 133 C): bash 4.4 and later print PS0
# once a command is read, before it runs.
PS0=$'\e]133;C\a'

# A terminal of a canvas finds its kou-canvas first.
if [[ -n ${KOU_CANVAS_BIN-} ]]; then
  PATH=$KOU_CANVAS_BIN:$PATH
fi
