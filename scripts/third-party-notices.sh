#!/usr/bin/env bash
# third-party-notices.sh prints the third-party notices of the programs built
# from the given packages, for them to ship with:
#
#   scripts/third-party-notices.sh ./cmd/kou-conveyor-runner > THIRD_PARTY_NOTICES.txt
#
# The notices are Go's license, for the runtime and standard library that
# every program links; for each module the packages link, on any of the
# platforms in $TARGETS (GOOS/GOARCH pairs: by default Linux and macOS on amd64
# and arm64), with cgo or without, the license files at its root and in the
# directories of the packages linked from it, and the address of its source;
# and the license files among what the packages embed, such as the fonts of
# the browser cockpit, or among the third-party sources they are generated
# from. A module without a license file fails it: such a dependency must not
# ship before it is looked into.
#
# It runs on macOS's Bash 3.2 as well.
set -euo pipefail
export LC_ALL=C

if [[ $# -eq 0 ]]; then
	echo "usage: $0 package..." >&2
	exit 2
fi
cd "$(dirname "$0")/.."
root=$PWD
targets=${TARGETS:-linux/amd64 linux/arm64 darwin/amd64 darwin/arm64}

# The packages of this module generated from third-party sources, whose
# licenses they carry: the package, the directory of its sources, and what
# they are.
generated=(
	"internal/openaiapi|third_party/openai-openapi|the OpenAI OpenAPI specification"
)

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# A line for each package linked: "main" and its import path for this module's
# packages, and "embed", the package and the file for each file they embed;
# "module", the module's path, version and directory and the package's
# directory for the others.
template='{{if not .Standard}}{{with .Module}}{{if .Main}}main{{"\t"}}{{$.ImportPath}}{{range $.EmbedFiles}}{{"\n"}}embed{{"\t"}}{{$.ImportPath}}{{"\t"}}{{$.Dir}}/{{.}}{{end}}{{else}}module{{"\t"}}{{with .Replace}}{{.Path}}{{"\t"}}{{.Version}}{{else}}{{.Path}}{{"\t"}}{{.Version}}{{end}}{{"\t"}}{{.Dir}}{{"\t"}}{{$.Dir}}{{end}}{{end}}{{end}}'
for target in $targets; do
	for cgo in 0 1; do
		GOOS=${target%/*} GOARCH=${target#*/} CGO_ENABLED=$cgo go list -deps -f "$template" "$@" || exit 1
	done
done | sed '/^$/d' | sort -u >"$work/packages"
module=$(go list -m)

# notices lists the license and notice files right in a directory.
notices() {
	find "$1" -maxdepth 1 -type f \( -iname 'licen[cs]e*' -o -iname 'copying*' -o -iname 'copyright*' \
		-o -iname 'notice*' -o -iname 'patents*' -o -iname 'unlicense*' \) ! -name '*.go' | sort
}

# escape writes a module path or version as the module mirror's addresses
# have it: an upper-case letter as "!" and the letter in lower case.
escape() {
	printf '%s\n' "$1" | awk '{
		out = ""
		for (i = 1; i <= length($0); i++) {
			c = substr($0, i, 1)
			out = out (c ~ /[A-Z]/ ? "!" tolower(c) : c)
		}
		print out
	}'
}

# section starts the notices of one piece of software: a rule, its name, and
# the lines that follow it.
section() {
	printf '\n%s\n' '================================================================================'
	printf '%s\n' "$@"
}

# license prints a license file under its name.
license() {
	printf '\n--- %s ---\n\n' "$2"
	cat "$1"
	if [[ -n $(tail -c 1 "$1") ]]; then
		echo
	fi
}

programs=
for package in "$@"; do
	programs+="${programs:+, }$(basename "$package")"
done
cat <<EOF
Third-party notices of $programs

Built from kou-conveyor, which is under the MIT License (see LICENSE), and from
the third-party software below, each under the license that follows its name:
Go; the Go modules linked in, each with the address of its source on the Go
module mirror; and the files embedded, or that code is generated from.
EOF

goversion=$(go env GOVERSION)
goversion=${goversion%% *}
section "Go ${goversion#go}" \
	"The runtime and the standard library, which every Go program links." \
	"Source: https://go.googlesource.com/go/+/refs/tags/$goversion"
goroot=$(go env GOROOT)
for name in LICENSE PATENTS; do
	license "$goroot/$name" "$name"
done

awk -F '\t' '$1 == "module" { print $2 "\t" $3 "\t" $4 "\t" $5 }' "$work/packages" >"$work/modules"
: >"$work/missing"
cut -f 1-3 "$work/modules" | sort -u | while IFS=$'\t' read -r path version dir; do
	if [[ -n $version ]]; then
		section "$path $version" "Source: https://proxy.golang.org/$(escape "$path")/@v/$(escape "$version").zip"
	else
		section "$path" "Source: this repository, at $path"
	fi
	awk -F '\t' -v path="$path" -v version="$version" '$1 == path && $2 == version { print $4 }' "$work/modules" |
		while read -r package; do
			while :; do
				notices "$package"
				if [[ $package == "$dir" || $package != "$dir"/* ]]; then
					break
				fi
				package=$(dirname "$package")
			done
		done | sort -u >"$work/files"
	if [[ ! -s $work/files ]]; then
		echo "$path $version" >>"$work/missing"
	fi
	while read -r found; do
		license "$found" "${found#"$dir"/}"
	done <"$work/files"
done

awk -F '\t' '$1 == "embed" { print $2 "\t" $3 }' "$work/packages" | while IFS=$'\t' read -r package found; do
	name=$(basename "$found" | tr '[:upper:]' '[:lower:]')
	# Text files alone: a script or a style named for notices is not one.
	case ${name##*.} in
	"$name" | txt | md) ;;
	*) continue ;;
	esac
	case $name in
	licen[cs]e* | copying* | copyright* | notice* | *notices* | patents* | unlicense* | ofl*) ;;
	*) continue ;;
	esac
	section "${found#"$root"/}" "Embedded in ${package#"$module"/}."
	license "$found" "$(basename "$found")"
done

for entry in "${generated[@]}"; do
	package=${entry%%|*}
	rest=${entry#*|}
	sources=${rest%%|*}
	what=${rest#*|}
	if ! awk -F '\t' -v package="$module/$package" '$1 == "main" && $2 == package { found = 1 } END { exit !found }' "$work/packages"; then
		continue
	fi
	found=$(notices "$sources")
	if [[ -z $found ]]; then
		echo "$sources: no license file for $package, which is generated from it" >&2
		exit 1
	fi
	section "$sources" "$package is generated from $what."
	while read -r name; do
		license "$name" "${name#"$sources"/}"
	done <<<"$found"
done

if [[ -s $work/missing ]]; then
	echo "these modules ship no license file:" >&2
	sed 's/^/  /' "$work/missing" >&2
	exit 1
fi
