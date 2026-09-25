#!/bin/sh
# GitGlance: the arguments arrive on standard input as JSON, and what this
# prints is the result the agent reads.
arguments=$(cat)
commits=$(printf '%s' "$arguments" | sed -n 's/.*"commits"[[:space:]]*:[[:space:]]*\([0-9][0-9]*\).*/\1/p')
[ -n "$commits" ] || commits=5

if ! git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo '{"error": "the workspace is not a git repository"}'
  exit 0
fi

# escape makes each line of its input a JSON string's content.
escape() { sed -e 's/\\/\\\\/g' -e 's/"/\\"/g' -e 's/	/\\t/g'; }

branch=$(git rev-parse --abbrev-ref HEAD 2>/dev/null | escape)
printf '{"branch": "%s", "changes": [' "$branch"
git status --porcelain=v1 | escape | awk '{ status = substr($0, 1, 2); gsub(/ /, "", status); printf "%s{\"status\": \"%s\", \"path\": \"%s\"}", (NR > 1 ? ", " : ""), status, substr($0, 4) }'
printf '], "commits": ['
if [ "$commits" -gt 0 ]; then
  git log -n "$commits" --format='%h%x09%s' 2>/dev/null | escape | awk -F '\\\\t' '{ printf "%s{\"hash\": \"%s\", \"subject\": \"%s\"}", (NR > 1 ? ", " : ""), $1, $2 }'
fi
printf ']}\n'
