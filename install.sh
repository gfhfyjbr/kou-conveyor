#!/bin/sh
# Install kou-conveyor: the runner and its terminal and browser cockpits.
#
#   sh install.sh                      from a checkout: build it
#   curl -fsSL https://raw.githubusercontent.com/gfhfyjbr/kou-conveyor/main/install.sh | sh
#
# Run with --help for options.
set -eu

REPO=gfhfyjbr/kou-conveyor
MODULE=github.com/$REPO
ALL="kou-conveyor-runner kou-conveyor-tui kou-conveyor-web"

dir=${KOU_CONVEYOR_INSTALL_DIR:-$HOME/.local/bin}
version=latest
method=auto
uninstall=0

usage() {
	cat <<EOF
Usage: install.sh [options]

Installs kou-conveyor-runner, kou-conveyor-tui and kou-conveyor-web.

Options:
  --dir DIR         install into DIR (default: \$KOU_CONVEYOR_INSTALL_DIR or ~/.local/bin)
  --version VER     release to install, e.g. v0.2.0 (default: latest)
  --method M        how to get the binaries:
                      checkout  build the checkout this script is in
                      go        go install $MODULE/cmd/...@VER
                      release   download the runner from GitHub releases
                      auto      the first that applies, in that order (default)
  --uninstall       remove the binaries from DIR
  -h, --help        show this help

Building needs Go 1.21 or newer, which fetches the Go release the project
requires by itself. Releases carry only the runner; the cockpits are built.
EOF
}

say() { printf '%s\n' "$*"; }
note() { printf '  %s\n' "$*"; }
die() {
	printf 'install.sh: %s\n' "$*" >&2
	exit 1
}
have() { command -v "$1" >/dev/null 2>&1; }

while [ $# -gt 0 ]; do
	case $1 in
	--dir) [ $# -ge 2 ] || die "--dir needs a directory"; dir=$2; shift 2 ;;
	--dir=*) dir=${1#*=}; shift ;;
	--version) [ $# -ge 2 ] || die "--version needs a version"; version=$2; shift 2 ;;
	--version=*) version=${1#*=}; shift ;;
	--method) [ $# -ge 2 ] || die "--method needs a value"; method=$2; shift 2 ;;
	--method=*) method=${1#*=}; shift ;;
	--uninstall) uninstall=1; shift ;;
	-h | --help) usage; exit 0 ;;
	*) die "unknown option $1 (see --help)" ;;
	esac
done

case $version in
latest | v[0-9]*) ;;
[0-9]*) version=v$version ;;
*) die "version must look like v0.2.0 or be latest" ;;
esac
case $method in
auto | checkout | go | release) ;;
*) die "method must be auto, checkout, go or release" ;;
esac
[ -n "$dir" ] || die "the install directory must not be empty"
case $dir in
"~") dir=$HOME ;;
"~"/*) dir=$HOME/${dir#"~/"} ;;
/*) ;;
*) dir=$(pwd)/$dir ;;
esac

if [ "$uninstall" = 1 ]; then
	removed=0
	for name in $ALL; do
		if [ -e "$dir/$name" ]; then
			rm -f "$dir/$name"
			note "removed $dir/$name"
			removed=1
		fi
	done
	[ "$removed" = 1 ] || say "Nothing to remove in $dir."
	say "Sessions, logs and settings stay where they are: <workspace>/.harness and the kou-conveyor settings file."
	exit 0
fi

# The checkout this script lives in, if it was run from one rather than piped.
checkout=
rerun="curl -fsSL https://raw.githubusercontent.com/$REPO/main/install.sh | sh -s --"
case $0 in
*install.sh)
	here=$(cd "$(dirname "$0")" && pwd)
	rerun="sh $0"
	if [ -f "$here/go.mod" ] && grep -q "^module $MODULE\$" "$here/go.mod"; then
		checkout=$here
	fi
	;;
esac

# go_usable reports whether a Go that can switch toolchains (1.21+) exists.
go_usable() {
	have go || return 1
	gov=$(go env GOVERSION 2>/dev/null) || return 1
	gov=${gov#go}
	major=${gov%%.*}
	rest=${gov#*.}
	minor=${rest%%[!0-9]*}
	[ "$major" -gt 1 ] 2>/dev/null && return 0
	[ "$major" -eq 1 ] 2>/dev/null && [ "${minor:-0}" -ge 21 ] 2>/dev/null
}

tmp=
cleanup() { [ -z "$tmp" ] || rm -rf "$tmp"; }
trap cleanup EXIT
trap 'exit 130' INT TERM
tmp=$(mktemp -d 2>/dev/null || mktemp -d -t kou-conveyor) || die "cannot create a temporary directory"

mkdir -p "$dir" || die "cannot create $dir"
installed=
absent=

# place moves a built binary into the install directory. A rename replaces
# the old file instead of rewriting it, so a copy that is running keeps working.
place() {
	if ! { cp "$1" "$dir/.$2.new" && chmod 755 "$dir/.$2.new" && mv -f "$dir/.$2.new" "$dir/$2"; }; then
		rm -f "$dir/.$2.new"
		die "cannot install $2 into $dir"
	fi
	installed="$installed $2"
}

# Each method returns non-zero when it installed nothing, so auto can fall
# back to the next one.
from_checkout() {
	go_usable || { say "Building needs Go 1.21 or newer: https://go.dev/dl/"; return 1; }
	[ "$version" = latest ] || say "Note: building the checkout as it is; --version applies to go and release installs."
	say "Building $checkout"
	for name in $ALL; do
		note "$name"
		# The browser cockpit follows the checkout it was built from: its page
		# and built-in plugins are read from there, live, while it is there.
		flags=
		[ "$name" != kou-conveyor-web ] || flags="-X 'main.builtFrom=$checkout'"
		if ! (cd "$checkout" && go build -trimpath -ldflags "$flags" -o "$tmp/$name" "./cmd/$name"); then
			say "Building $name failed."
			return 1
		fi
	done
	for name in $ALL; do
		place "$tmp/$name" "$name"
	done
}

from_go() {
	go_usable || { say "go install needs Go 1.21 or newer: https://go.dev/dl/"; return 1; }
	say "Installing $MODULE@$version with go install"
	for name in $ALL; do
		note "$name"
		if ! GOBIN=$tmp go install "$MODULE/cmd/$name@$version" 2>"$tmp/$name.log"; then
			if grep -q "does not contain package" "$tmp/$name.log"; then
				absent="$absent $name" # not part of that release yet
			else
				sed 's/^/    /' "$tmp/$name.log" >&2
				return 1
			fi
		fi
	done
	for name in $ALL; do
		[ ! -f "$tmp/$name" ] || place "$tmp/$name" "$name"
	done
	[ -n "$installed" ]
}

from_release() {
	have curl || { say "Downloading a release needs curl."; return 1; }
	have tar || { say "Unpacking a release needs tar."; return 1; }
	case $(uname -s) in
	Linux) os=linux ;;
	Darwin) os=darwin ;;
	*) say "Releases are built for Linux and macOS; install Go and use --method go."; return 1 ;;
	esac
	case $(uname -m) in
	x86_64 | amd64) arch=amd64 ;;
	arm64 | aarch64) arch=arm64 ;;
	*) say "Releases are built for amd64 and arm64; install Go and use --method go."; return 1 ;;
	esac
	tag=$version
	if [ "$tag" = latest ]; then
		url=$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/$REPO/releases/latest") ||
			{ say "Cannot reach GitHub to find the latest release."; return 1; }
		tag=${url##*/}
		case $tag in
		v[0-9]*) ;;
		*) say "No release found at https://github.com/$REPO/releases"; return 1 ;;
		esac
	fi
	archive=kou-conveyor-runner_${tag#v}_${os}_${arch}.tar.gz
	base=https://github.com/$REPO/releases/download/$tag
	say "Downloading $archive"
	curl -fsSL -o "$tmp/$archive" "$base/$archive" || { say "Cannot download $base/$archive"; return 1; }
	curl -fsSL -o "$tmp/SHA256SUMS" "$base/SHA256SUMS" || { say "Cannot download the release checksums."; return 1; }
	want=$(awk -v file="$archive" '{ name = $2; sub(/^\*?\.\//, "", name); if (name == file) print $1 }' "$tmp/SHA256SUMS")
	[ -n "$want" ] || die "SHA256SUMS does not list $archive"
	if have sha256sum; then
		got=$(sha256sum "$tmp/$archive" | awk '{ print $1 }')
	elif have shasum; then
		got=$(shasum -a 256 "$tmp/$archive" | awk '{ print $1 }')
	else
		die "verifying the download needs sha256sum or shasum"
	fi
	[ "$got" = "$want" ] || die "checksum mismatch for $archive; not installing it"
	mkdir "$tmp/unpacked"
	tar -xzf "$tmp/$archive" -C "$tmp/unpacked" || die "cannot unpack $archive"
	for name in $ALL; do
		[ ! -f "$tmp/unpacked/$name" ] || place "$tmp/unpacked/$name" "$name"
	done
	[ -n "$installed" ] || die "$archive holds no kou-conveyor binaries"
}

case $method in
checkout)
	[ -n "$checkout" ] || die "--method checkout needs this script inside a checkout of $REPO"
	from_checkout || die "the checkout was not installed"
	;;
go) from_go || die "go install failed" ;;
release) from_release || die "the release was not installed" ;;
auto)
	if [ -n "$checkout" ] && from_checkout; then
		:
	elif go_usable && from_go; then
		:
	else
		[ -z "$installed" ] || die "stopped after installing:$installed"
		say "Falling back to the release download."
		from_release || die "nothing was installed"
	fi
	;;
esac

say ""
say "Installed into $dir:"
for name in $installed; do
	note "$name"
done
missing=
for name in $ALL; do
	case " $installed " in
	*" $name "*) ;;
	*) missing="$missing $name" ;;
	esac
done
if [ -n "$missing" ]; then
	say ""
	if [ -n "$absent" ]; then
		say "$version does not include$absent yet. Building a checkout of main does:"
		note "git clone https://github.com/$REPO && sh kou-conveyor/install.sh"
	else
		say "Releases carry only the runner. For$missing, install Go 1.21+"
		say "(https://go.dev/dl/) and run:"
		note "$rerun --method go"
	fi
fi

case ":$PATH:" in
*":$dir:"*) ;;
*)
	say ""
	say "$dir is not on your PATH. Add it, then open a new shell:"
	case ${SHELL:-} in
	*/fish) note "fish_add_path $dir" ;;
	*/zsh) note "echo 'export PATH=\"$dir:\$PATH\"' >> ~/.zshrc" ;;
	*) note "echo 'export PATH=\"$dir:\$PATH\"' >> ~/.profile" ;;
	esac
	;;
esac

if [ -n "$checkout" ] && [ "$method" != go ] && [ "$method" != release ] && case " $installed " in *" kou-conveyor-web "*) true ;; *) false ;; esac; then
	say ""
	say "kou-conveyor-web reads its page and built-in plugins from $checkout/cmd/kou-conveyor-web"
	say "while that is there: changes to them show in open pages at once (-assets embedded uses the copies built in)."
fi

say ""
say "Next:"
case " $installed " in
*" kou-conveyor-tui "*)
	note "kou-conveyor-tui -workspace .    terminal cockpit; /settings picks the API, base URL and key"
	note "kou-conveyor-web -workspace .    browser cockpit on http://127.0.0.1:8080"
	note "kou-conveyor-runner -p 'Summarize this project.'"
	note "Without settings, runs use OPENAI_API_KEY, or ANTHROPIC_API_KEY with KOU_CONVEYOR_LLM_PROVIDER=anthropic."
	;;
*)
	note "export OPENAI_API_KEY=..."
	note "kou-conveyor-runner -p 'Summarize this project.'"
	;;
esac
