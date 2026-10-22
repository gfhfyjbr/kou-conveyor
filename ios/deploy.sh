#!/bin/bash
# Build KouConveyor and install it on your iPhone, wherever the phone is.
#
#   ios/deploy.sh               build, sign and install
#   ios/deploy.sh --check       say how the phone can be reached; change nothing
#   ios/deploy.sh --build-only  build and sign with the profile Xcode has; no install
#   ios/deploy.sh --fresh       fetch a new signing profile even if the cached one is new
#
# The phone is reached the first way that works:
#   1. devicectl: the phone on a cable or on the same Wi-Fi as this Mac;
#   2. lockdown over the network: the phone anywhere, with the Tailscale app
#      connected on it and Tailscale running here, or over any other VPN between
#      the two (KOU_IPHONE_HOST). deploy_tcp.py installs over TCP 62078 with
#      pymobiledevice3 and this Mac's pairing with the phone.
#
# A free Apple ID signs for 7 days, and Xcode reuses the profile it cached, so a
# reinstall alone does not move the date. When that profile has less than
# KOU_IOS_PROFILE_MIN_DAYS (6) days left, or with --fresh, it is set aside before
# the build and Xcode fetches a new one. That happens only once the phone is known
# to be reachable; if the build with a new profile fails, the old one goes back
# and the app is built with it.
#
# Environment: KOU_IPHONE (the phone's name or UDID; default: the paired iPhone),
# KOU_IPHONE_HOST (its address over the VPN; default: its Tailscale IP),
# KOU_IOS_TEAM (default: the team of the Apple Development certificate).
set -euo pipefail
cd "$(dirname "$0")"

die() {
  echo "deploy: $*" >&2
  exit 1
}

mode=deploy fresh=0
for arg in "$@"; do
  case $arg in
    --check) mode=check ;;
    --build-only) mode=build ;;
    --fresh) fresh=1 ;;
    -h | --help)
      sed -n '2,/^set -e/{/^set -e/d; s/^# \{0,1\}//; p;}' "$(basename "$0")"
      exit 0
      ;;
    *) die "unknown argument $arg (see --help)" ;;
  esac
done

bundle=$(awk '/PRODUCT_BUNDLE_IDENTIFIER:/ { print $2; exit }' project.yml)
app=build/DD-dev/Build/Products/Debug-iphoneos/KouConveyor.app
profiles="$HOME/Library/Developer/Xcode/UserData/Provisioning Profiles"
min_days=${KOU_IOS_PROFILE_MIN_DAYS:-6}
log=${TMPDIR:-/tmp}
log=${log%/}/kou-ios-deploy.log
devices=$(mktemp -t kou-devices)
stash=""

# expiry <profile>: when the profile, if it is the app's, runs out, in Unix time.
expiry() {
  security cms -D -i "$1" 2>/dev/null | python3 -c '
import calendar, plistlib, sys
p = plistlib.loads(sys.stdin.buffer.read())
ok = p.get("Entitlements", {}).get("application-identifier", "").endswith("." + sys.argv[1])
print(calendar.timegm(p["ExpirationDate"].timetuple()) if ok else "")' "$bundle"
}

# cached: "<expiry> <path>" of each profile Xcode keeps for the app.
cached() {
  local f e
  for f in "$profiles"/*.mobileprovision; do
    [ -f "$f" ] || continue
    e=$(expiry "$f") || continue
    if [ -n "$e" ]; then echo "$e $f"; fi
  done
  return 0
}

when() { date -r "$1" '+%d.%m %H:%M'; }

# lockdown_at <host>: whether the phone's lockdownd answers on host:62078, and why
# not when it does not. A bare connect proves nothing: a VPN on this Mac that
# proxies all TCP accepts every connection itself.
lockdown_at() {
  python3 - "$1" <<'PY'
import plistlib, socket, struct, sys

host = sys.argv[1]
reply = {}
try:
    with socket.create_connection((host, 62078), timeout=8) as s:
        s.settimeout(8)
        query = plistlib.dumps({"Label": "kou-deploy", "Request": "QueryType"})
        s.sendall(struct.pack(">I", len(query)) + query)
        data = b""
        while len(data) < 4 or len(data) < 4 + struct.unpack(">I", data[:4])[0]:
            chunk = s.recv(65536)
            if not chunk:
                break
            data += chunk
        reply = plistlib.loads(data[4:])
except ConnectionRefusedError:
    sys.exit(f"{host}:62078 refuses: the phone is there, but its lockdown takes no network connections")
except OSError:
    sys.exit(f"{host}:62078 does not answer: Tailscale is off on the phone, or the phone is out of reach")
except Exception:
    pass
if reply.get("Type") != "com.apple.mobile.lockdown":
    sys.exit(f"{host}:62078 is not the phone's lockdown: a VPN on this Mac may catch the address")
PY
}

# restore: put the profiles set aside back.
restore() {
  [ -n "$stash" ] || return 0
  mv "$stash"/*.mobileprovision "$profiles"/ 2>/dev/null || true
  rmdir "$stash" 2>/dev/null || true
  stash=""
}
trap 'restore; rm -f "$devices"' EXIT
trap 'exit 130' INT TERM HUP

team=${KOU_IOS_TEAM:-}
if [ -z "$team" ]; then
  team=$(security find-certificate -c "Apple Development" -p 2>/dev/null |
    openssl x509 -noout -subject -nameopt multiline 2>/dev/null |
    awk -F' = ' '/organizationalUnitName/ { print $2; exit }') || true
fi

# build: generate the project, then build and sign the app; the log says why not.
build() {
  [ -n "$team" ] || die "no signing team: sign in to Xcode with your Apple ID, or set KOU_IOS_TEAM"
  : >"$log"
  echo "building KouConveyor… (log: $log)"
  xcodegen generate --quiet >>"$log" 2>&1 &&
    xcodebuild -project KouConveyor.xcodeproj -scheme KouConveyor -configuration Debug \
      -destination 'generic/platform=iOS' -derivedDataPath build/DD-dev \
      -allowProvisioningUpdates DEVELOPMENT_TEAM="$team" build >>"$log" 2>&1
}

# broken: show why the build failed, and stop.
broken() {
  if grep -q -E 'error:|BUILD FAILED' "$log"; then
    grep -E 'error:|BUILD FAILED' "$log" | awk '!seen[$0]++' | tail -25 >&2
  else
    tail -20 "$log" >&2
  fi
  die "the build failed; the whole log is $log"
}

# built: what the build made, and until when it is signed.
built() {
  local v b
  v=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$app/Info.plist")
  b=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleVersion' "$app/Info.plist")
  echo "KouConveyor $v ($b), signed until $(when "$(expiry "$app/embedded.mobileprovision")")"
}

if [ "$mode" = build ]; then
  build || broken
  echo "built $(built)"
  exit 0
fi

# The phone, as devicectl knows it: paired with this Mac, reachable or not.
xcrun devicectl list devices --json-output "$devices" >/dev/null 2>&1 || true
device=$(jq -c --arg want "${KOU_IPHONE:-}" '
  [.result.devices[]?
   | select(.hardwareProperties.platform == "iOS" and .connectionProperties.pairingState == "paired")
   | select($want == "" or $want == .deviceProperties.name or $want == .hardwareProperties.udid or $want == .identifier)]
  | first // empty' "$devices" 2>/dev/null) || true
[ -n "$device" ] || die "no iPhone${KOU_IPHONE:+ named $KOU_IPHONE} is paired with this Mac: connect it by cable once and tap Trust"
name=$(jq -r '.deviceProperties.name' <<<"$device")
udid=$(jq -r '.hardwareProperties.udid' <<<"$device")
id=$(jq -r '.identifier' <<<"$device")
tunnel=$(jq -r '.connectionProperties.tunnelState // "unavailable"' <<<"$device")

# How to reach it now.
route="" host=${KOU_IPHONE_HOST:-} why=()
if [ "$tunnel" != unavailable ]; then
  route=devicectl
else
  why+=("devicectl does not see it: not on a cable or this Wi-Fi")
  if [ -z "$host" ]; then
    # Tailscale's state, from Homebrew's tailscaled or from the Tailscale app.
    ts="" state=""
    for cli in tailscale /Applications/Tailscale.app/Contents/MacOS/Tailscale; do
      command -v "$cli" >/dev/null || continue
      ts=$("$cli" status --json 2>/dev/null) || true
      state=$(jq -r '.BackendState // empty' <<<"$ts" 2>/dev/null) || true
      if [ -n "$state" ]; then break; fi
    done
    if [ -z "$state" ]; then
      why+=("Tailscale does not run on this Mac: sudo brew services start tailscale, then sudo tailscale up")
    elif [ "$state" != Running ]; then
      why+=("Tailscale on this Mac is $state: run sudo tailscale up")
    else
      slug=$(echo "$name" | tr '[:upper:]' '[:lower:]' | tr ' ' '-')
      host=$(jq -r --arg slug "$slug" '
        [.Peer[]? | select(.Online and .OS == "iOS")] as $ios
        | ($ios | map(select((.DNSName // "" | ascii_downcase | startswith($slug + "."))
            or (((.HostName // "") | ascii_downcase | gsub(" "; "-")) == $slug)))) as $named
        | if ($named | length) == 1 then $named[0].TailscaleIPs[0] // empty
          elif ($ios | length) == 1 then $ios[0].TailscaleIPs[0] // empty
          else empty end' <<<"$ts")
      [ -n "$host" ] || why+=("no online iPhone in the tailnet: connect Tailscale on the phone (or set KOU_IPHONE_HOST)")
    fi
  fi
  if [ -n "$host" ]; then
    if why_not=$(lockdown_at "$host" 2>&1); then
      route=lockdown
    else
      why+=("$why_not")
    fi
  fi
fi

# Whether the deploy renews the signature.
newest=$(cached | sort -n | tail -1)
renew=$fresh
if [ -z "$newest" ] || [ $((${newest%% *} - $(date +%s))) -lt $((min_days * 86400)) ]; then renew=1; fi

if [ "$mode" = check ]; then
  echo "phone: $name, iOS $(jq -r '.deviceProperties.osVersionNumber // "?"' <<<"$device"), developer mode $(jq -r '.deviceProperties.developerModeStatus // "?"' <<<"$device")"
  case $route in
    devicectl) echo "route: devicectl (cable or this Wi-Fi)" ;;
    lockdown) echo "route: lockdown over the network at $host" ;;
    *)
      echo "route: none"
      printf '  - %s\n' "${why[@]}"
      ;;
  esac
  if [ -z "$newest" ]; then
    echo "signing: no cached profile; the deploy fetches one (7 days)"
  elif [ "$renew" = 1 ]; then
    echo "signing: the cached profile runs out $(when "${newest%% *}"); the deploy fetches a new one (7 days)"
  else
    echo "signing: the cached profile runs out $(when "${newest%% *}"); the deploy keeps it"
  fi
  [ -n "$route" ] || exit 1
  exit 0
fi

if [ -z "$route" ]; then
  printf '  - %s\n' "${why[@]}" >&2
  die "cannot reach $name now, so nothing was built or changed"
fi
if [ "$route" = lockdown ] && ! command -v uv >/dev/null; then
  die "uv is needed to install over the network: brew install uv"
fi

before=""
if [ "$renew" = 1 ] && [ -n "$newest" ]; then
  before=${newest%% *}
  stash=$(mktemp -d -t kou-profiles)
  cached | while read -r _ path; do mv "$path" "$stash/"; done
  echo "set the cached signing profile aside; Xcode fetches a new one"
fi
if ! build; then
  [ -n "$stash" ] || broken
  restore
  before=""
  echo "deploy: the build with a new profile failed; building with the old one, so the date does not move" >&2
  build || broken
fi
if [ -n "$stash" ]; then
  rm -rf "$stash"
  stash=""
fi
after=$(expiry "$app/embedded.mobileprovision") || true
if [ -n "$before" ] && [ -n "$after" ] && [ "$after" -le "$before" ]; then
  echo "deploy: warning: Xcode signed with the old profile again; the date did not move" >&2
fi

case $route in
  devicectl)
    echo "installing on $name with devicectl…"
    xcrun devicectl device install app --device "$id" "$app" >>"$log" 2>&1 || {
      tail -15 "$log" >&2
      die "devicectl could not install it"
    }
    ;;
  lockdown)
    echo "installing on $name over the network ($host)…"
    uv run --quiet --script deploy_tcp.py "$host" "$udid" "$app" || die "the install over the network failed"
    ;;
esac
echo "installed $(built) on $name"
