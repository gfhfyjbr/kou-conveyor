---
name: kou-conveyor-tunnel
description: >-
  Set up, check and repair the tunnel of the kou-conveyor browser cockpit:
  install kou-conveyor-relay on a host over ssh, so that the kou-conveyor
  iPhone app and browsers elsewhere reach the cockpit through it. Use when
  the user runs /tunnel, asks to reach the cockpit from the phone or from
  another machine, or the tunnel does not come up.
---

# The cockpit's tunnel

The browser cockpit (kou-conveyor-web) runs on this machine and listens on
loopback only. The tunnel carries it to the kou-conveyor iPhone app and to
browsers elsewhere: a small relay, `kou-conveyor-relay`, runs on a host they
reach; the cockpit connects *out* to it over a WebSocket, and each request a
client with the relay's token makes goes down that connection to the
cockpit, which serves it as it serves its own pages. The relay keeps nothing
but its token and its certificate.

`tunnel.json` — its path is in `$KOU_CONVEYOR_TUNNEL_CONFIG` — says where the
relay is: its URL, its token, the fingerprint of its certificate, the ssh
host it was installed on, and whether the tunnel is on. The cockpit follows
the file while it runs: what the commands below write there, it takes up
within seconds. Never show the token in your answers; the user pairs the
phone with the code in the cockpit's tunnel dialog.

The commands are `"$KOU_CONVEYOR_WEB" tunnel …` — `$KOU_CONVEYOR_WEB` is the
cockpit's own program (`kou-conveyor-web` on the PATH is the same):

| Command | What it does |
| --- | --- |
| `tunnel status` (`-json`) | where the relay is and whether it answers; exit 1 and a `code` when it does not |
| `tunnel install DEST` | installs the relay on DEST over ssh, keeps it running, writes tunnel.json; `-dry-run` only looks |
| `tunnel set -url URL -token T [-fingerprint FP]` | points the tunnel at a relay set up by other means |
| `tunnel start`, `tunnel stop` | turns the tunnel on or off |
| `tunnel link` | the link that pairs the iPhone app (`-browser`: the one that opens the cockpit in a browser) |

## Setting it up

1. Run `"$KOU_CONVEYOR_WEB" tunnel status`. When it passes, the tunnel is
   set up: say so, and how to pair the phone (step 7).
2. With no relay (`not_configured`), ask the user which host to install it
   on, unless they said: an ssh destination — `user@host`, or a `Host` of
   their `~/.ssh/config` — that logs in with a key. The phone must reach the
   host: a server with a public address, or a machine on the same Tailscale
   network as the phone. Ask whether port 8420 suits them. Ask in one
   message, and end your turn to wait for the answer.
3. Check that ssh logs in without asking anything:
   `ssh -o BatchMode=yes DEST true`. If it does not, tell the user what to
   do (`ssh-copy-id DEST`, or add the key to their ssh agent); never type or
   ask for passwords.
4. Look first: `"$KOU_CONVEYOR_WEB" tunnel install DEST -dry-run`. Then
   install: the same without `-dry-run`. It finds the host's system and
   architecture, builds the relay for it from the kou-conveyor checkout (or
   uploads `-binary FILE`), puts it in `~/.local/bin` there, has it make its
   token and a certificate of its own in `~/.config/kou-conveyor-relay`, runs
   it as a systemd service (a user service unless DEST is root) or with
   nohup, waits until it answers from here, and writes tunnel.json. Its
   options: `-port N`; `-url https://NAME:PORT` when clients reach the host by
   another name or address than ssh does (its public IP, a domain, a
   Tailscale name); `-tls off` only behind a TLS proxy or on Tailscale;
   `-ssh "-p 2222 -i ~/.ssh/key"`; `-json` for a report to read.
5. When it says the relay runs on the host but does not answer from here,
   the port is closed between: the host's firewall (`sudo ufw allow
   8420/tcp`, or firewalld) or the provider's security group. Tell the user;
   ask before you run anything with sudo on their host.
6. Done when `"$KOU_CONVEYOR_WEB" tunnel status` passes. The cockpit
   connects by itself within seconds, and the planet beside the mark in its
   rail says "tunnelling".
7. Tell the user how to pair the iPhone app: in the cockpit, click the
   planet beside the mark, then **Show code**, and scan it with the phone's
   camera; or open the link `tunnel link` prints on the phone. Browsers
   elsewhere: **Copy browser link** in the same dialog; with the relay's own
   certificate a browser warns once.

## When it does not come up

`"$KOU_CONVEYOR_WEB" tunnel status -json` gives a `code`:

- `unreachable` — the relay does not answer. On the host:
  `systemctl --user status kou-conveyor-relay` (as root, without `--user`),
  `journalctl --user -u kou-conveyor-relay -n 50`, or
  `~/.config/kou-conveyor-relay/relay.log` when it runs with nohup;
  `curl -sk https://127.0.0.1:8420/_relay/health` there tells a stopped relay
  from a closed port. A relay run with nohup does not outlive a restart of
  the host: install again where systemd is.
- `unauthorized` — tunnel.json's token is not the relay's: install again, or
  `tunnel set -token "$(ssh DEST cat .config/kou-conveyor-relay/token)"`.
- `not_relay` — something else answers at that URL: another port, `-url`.
- `incompatible` — the relay and the cockpit are of different versions:
  install again.
- A certificate that is "not the one pinned": the relay made a new one;
  install again to take its fingerprint.

To move it to another host: `tunnel install NEWDEST`; the cockpit goes to the
new relay by itself. To turn it off: `tunnel stop`; the relay stays on its
host (`systemctl --user disable --now kou-conveyor-relay` there stops it).

## Security

The cockpit runs commands in the user's workspaces: whoever has the relay's
token commands this machine. The token is 256 random bits and crosses the
network only over TLS; with the relay's own certificate, the cockpit and the
app pin it. Keep the token out of answers, files in the workspace, commits,
issues and logs, and do not use `-tls off` on a public address.
