# Running roon-rd under launchd (home server)

A stop-gap until the home Kubernetes setup takes over: roon-rd as a per-user LaunchAgent.
It starts at login, restarts if it exits, and logs to `~/srv/roon-rd.log`.

Layout on the server (unchanged from the tmux setup):

```
~/srv/
  bin/roon-rd-macos-arm64     the binary
  config.toml                 (if used) read from the working directory
  roon-rd.log                 stdout + stderr, appended
```

## Install (once)

1. Stop the tmux copy: attach, Ctrl-C, exit the session. Only one roon-rd can own port 3000.
2. Copy the plist and check it:
   ```
   cp com.jdr.roon-rd.plist ~/Library/LaunchAgents/
   plutil -lint ~/Library/LaunchAgents/com.jdr.roon-rd.plist
   ```
3. Load and start it:
   ```
   launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.jdr.roon-rd.plist
   ```
4. Check it:
   ```
   launchctl print gui/$(id -u)/com.jdr.roon-rd | grep -E 'state|pid|last exit'
   tail -f ~/srv/roon-rd.log
   ```
   The first log line is the version line. Then confirm it finds the Roon Core and that
   http://<server>:3000 loads.

**Local Network permission.** Started by launchd, the binary may need its own permission
to reach devices on the LAN (under tmux it used the terminal's). Watch for a prompt on the
server's screen, or look in System Settings › Privacy & Security › Local Network. If the log
shows it can't find the Core, this is the likely cause; allow it, then restart (below).

Symptoms while it's missing: "No route to host (os error 65)" on every Roon discovery
attempt, and `roon-rd.local` doesn't resolve from other machines (ping: unknown host).
After allowing it, `kickstart -k` once: roon-rd registers its mDNS name only at startup.

**Over SSH:** `bootstrap` (and sometimes `bootout`) into `gui/<uid>` fails with
"Bootstrap failed: 125: Domain does not support specified action", because an SSH session
isn't part of the login session that owns that domain. Either run the command in Terminal
on the server's own screen (Screen Sharing), or run it inside the login session from SSH:

```
sudo launchctl asuser $(id -u) launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.jdr.roon-rd.plist
```

The same prefix works for any other command below that fails with 125. Once the plist is
in `~/Library/LaunchAgents`, it loads by itself at every login, so this is only needed for
the first start and after editing the plist.

## Day to day

| To | Run |
|---|---|
| restart (e.g. after copying in a new binary) | `launchctl kickstart -k gui/$(id -u)/com.jdr.roon-rd` |
| stop until next login | `launchctl bootout gui/$(id -u)/com.jdr.roon-rd` |
| start again | `launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.jdr.roon-rd.plist` |
| status | `launchctl print gui/$(id -u)/com.jdr.roon-rd` |
| follow the log | `tail -f ~/srv/roon-rd.log` |

After editing the plist: `bootout`, then `bootstrap` (kickstart doesn't reread it).

Updating the binary:

```
install -m 755 roon-rd-macos-arm64 ~/srv/bin/roon-rd-macos-arm64
xattr -d com.apple.quarantine ~/srv/bin/roon-rd-macos-arm64 2>/dev/null
~/srv/bin/roon-rd-macos-arm64 --version
launchctl kickstart -k gui/$(id -u)/com.jdr.roon-rd
```

- `install -m 755` sets the execute bit (without it launchd fails with exit code 78,
  `EX_CONFIG`, and retries every 10 s: "spawn scheduled"). It also writes a new file rather
  than overwriting in place; overwriting a running binary can get the process killed for
  an invalid code signature.
- A quarantined binary (downloaded, AirDropped) can hang waiting on a Gatekeeper dialog on
  the server's screen, invisible over SSH. `xattr -l` shows it; `com.apple.provenance` is
  harmless.
- `--version` by hand proves the binary runs before launchd tries it.
- If launchd ever failed to execute it ("Could not find and/or execute program", then
  "Missing executable detected" in `log show`), it stops retrying: the status stays
  "spawn scheduled" and `kickstart` hangs. Fixing the file isn't enough; reload the job
  with `bootout` then `bootstrap` (with the `asuser` prefix over SSH). If `bootout` hangs,
  `launchctl remove com.jdr.roon-rd` does the same.
- A replaced binary may need its Local Network permission granted again.

## The log

launchd appends and never rotates. Rotating with newsyslog doesn't work cleanly here:
roon-rd keeps the file open and has no signal to reopen it, so it would carry on writing
into the rotated file. To start a fresh log, move it aside and restart:

```
mv ~/srv/roon-rd.log ~/srv/roon-rd.$(date +%Y%m%d).log
launchctl kickstart -k gui/$(id -u)/com.jdr.roon-rd
```

## Remove (when moving to Kubernetes)

```
launchctl bootout gui/$(id -u)/com.jdr.roon-rd
rm ~/Library/LaunchAgents/com.jdr.roon-rd.plist
```
