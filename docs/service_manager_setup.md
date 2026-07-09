# Service Manager Setup

`filer` and `mounty` run in the foreground and don't daemonize themselves. The
platform service manager (`systemd` on Linux, `launchd` on macOS) owns
backgrounding, restarts, and logs. On `SIGTERM`, `filer` drains via Axum's
graceful shutdown and `mounty` unmounts the FUSE session before exiting.

## Quick Start

```bash
./deploy/install.sh
```

Interactive: builds release binaries, detects `systemd`/`launchd`, prompts for
paths and users, then installs and starts both services.

| Flag | Effect |
| --- | --- |
| `--dry-run` | Print actions/generated files, install nothing. |
| `--yes` | Accept defaults, skip prompts. |
| `--no-build` | Skip the `cargo build --release` step. |
| `--no-start` | Install service files without starting them. |
| `--manager systemd\|launchd` | Force a specific service manager. |

Uninstall:

```bash
./deploy/uninstall.sh            # stop/disable services, remove service files
./deploy/uninstall.sh --dry-run
```

Data directories, binaries, mountpoints, and logs are left in place unless you
pass `--remove-binaries`, `--remove-mount`, `--remove-logs`, or confirm
interactively.

## mounty Ownership

FUSE has two ownership layers:

- the **service user/group** creates the mount (`user_id`/`group_id` as seen by `findmnt`);
- `MOUNTY_UID`/`MOUNTY_GID` control what `ls`/`stat` show *inside* the mount.

The installer asks which user/group should run `mounty` (default: whoever
invoked the installer) and, by default, sets `MOUNTY_UID`/`MOUNTY_GID` to
match. Keeping the two aligned avoids the mount silently appearing as
`root:root`.

## Manual Install

Build and install the binaries:

```bash
cargo build --release --manifest-path filer/Cargo.toml
cargo build --release --manifest-path mounty/Cargo.toml
sudo install -m 0755 filer/target/release/remote-fs-server /usr/local/bin/remote-fs-server
sudo install -m 0755 mounty/target/release/mounty /usr/local/bin/mounty
```

If you install binaries somewhere else, update `ExecStart`/`ProgramArguments`
in the templates below to match.

### Linux (systemd)

Templates: `deploy/systemd/filer.service`, `deploy/systemd/mounty.service`

| | Default |
| --- | --- |
| Served directory | `/var/lib/rusty-fs/data` |
| Mountpoint | `/mnt/rusty-fs` |
| filer URL (used by mounty) | `http://127.0.0.1:3000` |

```bash
sudo mkdir -p /var/lib/rusty-fs/data /mnt/rusty-fs
sudo cp deploy/systemd/filer.service deploy/systemd/mounty.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now filer.service mounty.service
```

Check status / stop:

```bash
systemctl status filer.service mounty.service
journalctl -u filer.service -u mounty.service -f
sudo systemctl stop mounty.service filer.service
```

Notes:

- the service user needs access to `/dev/fuse` — install `fuse3` and add the
  user to the `fuse` group, or grant equivalent privileges;
- `mounty.service`'s `ExecStopPost=-/bin/fusermount3 -u /mnt/rusty-fs` is a
  best-effort fallback only; normal shutdown goes through `SIGTERM`;
- to point `mounty` at a remote `filer`, edit the URL in `mounty.service`'s
  `ExecStart` and drop `filer.service` from `After=`;
- for manual installs, set `User=`/`Group=` in `mounty.service` to the account
  that should own the mount, and matching `MOUNTY_UID`/`MOUNTY_GID`.

### macOS (launchd)

Templates: `deploy/launchd/com.rusty-fs.filer.plist`, `deploy/launchd/com.rusty-fs.mounty.plist`

| | Default |
| --- | --- |
| Served directory | `/usr/local/var/rusty-fs/data` |
| Mountpoint | `/Volumes/rusty-fs` |
| Logs | `/usr/local/var/log/rusty-fs` |
| filer URL (used by mounty) | `http://127.0.0.1:3000` |

Install macFUSE first, then:

```bash
sudo mkdir -p /usr/local/var/rusty-fs/data /usr/local/var/log/rusty-fs /Volumes/rusty-fs
sudo cp deploy/launchd/com.rusty-fs.filer.plist deploy/launchd/com.rusty-fs.mounty.plist /Library/LaunchDaemons/
sudo chown root:wheel /Library/LaunchDaemons/com.rusty-fs.*.plist
sudo chmod 0644 /Library/LaunchDaemons/com.rusty-fs.*.plist
sudo launchctl bootstrap system /Library/LaunchDaemons/com.rusty-fs.filer.plist
sudo launchctl bootstrap system /Library/LaunchDaemons/com.rusty-fs.mounty.plist
```

Stop:

```bash
sudo launchctl bootout system /Library/LaunchDaemons/com.rusty-fs.mounty.plist
sudo launchctl bootout system /Library/LaunchDaemons/com.rusty-fs.filer.plist
```

Notes:

- to point `mounty` at a remote `filer`, edit the URL in
  `com.rusty-fs.mounty.plist`;
- for manual installs, set `UserName`/`GroupName` in
  `com.rusty-fs.mounty.plist` to the account that should own the mount, and
  matching `MOUNTY_UID`/`MOUNTY_GID`;
- `ExitTimeOut` gives both processes time to finish graceful shutdown before
  `launchd` force-kills them on unload.
