# Service Manager Setup

`filer` and `mounty` are designed to run as foreground processes. They do not
daemonize themselves with fork/detach logic. Background operation is provided by
the platform service manager:

- `systemd` on Linux;
- `launchd` on macOS.

This keeps shutdown behavior explicit: the service manager sends `SIGTERM`,
`filer` drains through Axum graceful shutdown, and `mounty` asks the FUSE session
to unmount before exiting.

## Install Binaries

Build release binaries:

```bash
cargo build --release --manifest-path filer/Cargo.toml
cargo build --release --manifest-path mounty/Cargo.toml
```

Install them where the service templates expect them:

```bash
sudo install -m 0755 filer/target/release/remote-fs-server /usr/local/bin/remote-fs-server
sudo install -m 0755 mounty/target/release/mounty /usr/local/bin/mounty
```

If you install the binaries somewhere else, update the `ExecStart` or
`ProgramArguments` paths in the templates.

## Linux: systemd

Templates:

- `deploy/systemd/filer.service`
- `deploy/systemd/mounty.service`

Default paths:

- served directory: `/var/lib/rusty-fs/data`;
- mountpoint: `/mnt/rusty-fs`;
- filer URL used by mounty: `http://127.0.0.1:3000`.

Create the directories:

```bash
sudo mkdir -p /var/lib/rusty-fs/data /mnt/rusty-fs
```

Make sure the service user can access `/dev/fuse` and the mountpoint. On many
Linux distributions this means installing `fuse3` and adding the service user to
the `fuse` group, or running the unit with privileges appropriate for your
deployment.

Install and start the services:

```bash
sudo cp deploy/systemd/filer.service /etc/systemd/system/filer.service
sudo cp deploy/systemd/mounty.service /etc/systemd/system/mounty.service
sudo systemctl daemon-reload
sudo systemctl enable --now filer.service
sudo systemctl enable --now mounty.service
```

Check status and logs:

```bash
systemctl status filer.service mounty.service
journalctl -u filer.service -u mounty.service -f
```

Stop normally:

```bash
sudo systemctl stop mounty.service
sudo systemctl stop filer.service
```

`mounty.service` relies on `SIGTERM` for normal shutdown. Its
`ExecStopPost=-/bin/fusermount3 -u /mnt/rusty-fs` line is only a best-effort
cleanup fallback after the process has exited.

If `mounty` connects to a remote `filer` instead of the local
`filer.service`, edit the URL in `ExecStart` and remove `filer.service` from the
`After=` line.

## macOS: launchd

Templates:

- `deploy/launchd/com.rusty-fs.filer.plist`
- `deploy/launchd/com.rusty-fs.mounty.plist`

Default paths:

- served directory: `/usr/local/var/rusty-fs/data`;
- mountpoint: `/Volumes/rusty-fs`;
- logs: `/usr/local/var/log/rusty-fs`;
- filer URL used by mounty: `http://127.0.0.1:3000`.

Create the directories:

```bash
sudo mkdir -p /usr/local/var/rusty-fs/data /usr/local/var/log/rusty-fs /Volumes/rusty-fs
```

Install macFUSE before loading `mounty`. If `mounty` connects to a remote
`filer`, edit the URL in `com.rusty-fs.mounty.plist`.

Install the launch daemons:

```bash
sudo cp deploy/launchd/com.rusty-fs.filer.plist /Library/LaunchDaemons/
sudo cp deploy/launchd/com.rusty-fs.mounty.plist /Library/LaunchDaemons/
sudo chown root:wheel /Library/LaunchDaemons/com.rusty-fs.*.plist
sudo chmod 0644 /Library/LaunchDaemons/com.rusty-fs.*.plist
```

Load and start:

```bash
sudo launchctl bootstrap system /Library/LaunchDaemons/com.rusty-fs.filer.plist
sudo launchctl bootstrap system /Library/LaunchDaemons/com.rusty-fs.mounty.plist
```

Stop and unload:

```bash
sudo launchctl bootout system /Library/LaunchDaemons/com.rusty-fs.mounty.plist
sudo launchctl bootout system /Library/LaunchDaemons/com.rusty-fs.filer.plist
```

`launchd` sends termination signals during unload. `ExitTimeOut` gives the
processes time to complete graceful shutdown before they are forcefully killed.

## What This Does Not Add

This does not add a `--daemon` flag, PID files, or in-process double-fork
daemonization. The supported model is:

1. `filer` and `mounty` run in the foreground.
2. The service manager owns background execution, restarts, logs, and stop
   signals.
3. Application code owns startup validation and graceful shutdown.
