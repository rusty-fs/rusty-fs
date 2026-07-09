#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat <<'EOF'
Usage: deploy/install.sh [options]

Interactive installer for rusty-fs service-manager integration.

Options:
  --dry-run       Print actions and generated service files without installing.
  --yes           Accept defaults and do not prompt.
  --no-build      Skip cargo release builds.
  --no-start      Install files but do not start services.
  --manager NAME  Force service manager: systemd or launchd.
  --help          Show this help.
EOF
}

DRY_RUN=0
ASSUME_YES=0
NO_BUILD=0
NO_START=0
MANAGER=""

while [ "$#" -gt 0 ]; do
    case "$1" in
        --dry-run) DRY_RUN=1 ;;
        --yes) ASSUME_YES=1 ;;
        --no-build) NO_BUILD=1 ;;
        --no-start) NO_START=1 ;;
        --manager)
            MANAGER="${2:-}"
            if [ -z "$MANAGER" ]; then
                echo "ERROR: --manager requires a value" >&2
                exit 1
            fi
            shift
            ;;
        --help|-h)
            usage
            exit 0
            ;;
        *)
            echo "ERROR: unknown option: $1" >&2
            usage
            exit 1
            ;;
    esac
    shift
done

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
WORK_DIR=""

cleanup() {
    if [ -n "$WORK_DIR" ] && [ -d "$WORK_DIR" ]; then
        rm -rf "$WORK_DIR"
    fi
}
trap cleanup EXIT

detect_manager() {
    if [ -n "$MANAGER" ]; then
        case "$MANAGER" in
            systemd|launchd) echo "$MANAGER" ;;
            *)
                echo "ERROR: unsupported manager: $MANAGER" >&2
                exit 1
                ;;
        esac
        return
    fi

    case "$(uname -s)" in
        Linux) echo "systemd" ;;
        Darwin) echo "launchd" ;;
        *)
            echo "ERROR: unsupported OS. Use Linux/systemd or macOS/launchd." >&2
            exit 1
            ;;
    esac
}

prompt() {
    local var_name="$1"
    local label="$2"
    local default_value="$3"
    local input_value

    if [ "$ASSUME_YES" -eq 1 ]; then
        printf -v "$var_name" '%s' "$default_value"
        return
    fi

    read -r -p "$label [$default_value]: " input_value
    if [ -z "$input_value" ]; then
        input_value="$default_value"
    fi
    printf -v "$var_name" '%s' "$input_value"
}

prompt_choice() {
    local var_name="$1"
    local label="$2"
    local default_value="$3"
    local choice_value

    while true; do
        prompt choice_value "$label" "$default_value"
        case "$choice_value" in
            both|filer|mounty)
                printf -v "$var_name" '%s' "$choice_value"
                return
                ;;
            *)
                echo "Please choose one of: both, filer, mounty"
                ;;
        esac
    done
}

prompt_yes_no() {
    local var_name="$1"
    local label="$2"
    local default_value="$3"
    local input_value

    if [ "$ASSUME_YES" -eq 1 ]; then
        case "$default_value" in
            y|Y|yes|YES) printf -v "$var_name" '1' ;;
            n|N|no|NO) printf -v "$var_name" '0' ;;
            *)
                echo "ERROR: invalid yes/no default: $default_value" >&2
                exit 1
                ;;
        esac
        return
    fi

    while true; do
        read -r -p "$label [$default_value]: " input_value
        if [ -z "$input_value" ]; then
            input_value="$default_value"
        fi

        case "$input_value" in
            y|Y|yes|YES)
                printf -v "$var_name" '1'
                return
                ;;
            n|N|no|NO)
                printf -v "$var_name" '0'
                return
                ;;
            *)
                echo "Please answer y or n"
                ;;
        esac
    done
}

run() {
    if [ "$DRY_RUN" -eq 1 ]; then
        printf '[dry-run] '
        printf '%q ' "$@"
        printf '\n'
    else
        "$@"
    fi
}

sudo_run() {
    if [ "$(id -u)" -eq 0 ]; then
        run "$@"
    else
        run sudo "$@"
    fi
}

sudo_run_may_fail() {
    if [ "$DRY_RUN" -eq 1 ]; then
        if [ "$(id -u)" -eq 0 ]; then
            run "$@"
        else
            run sudo "$@"
        fi
        return
    fi

    if [ "$(id -u)" -eq 0 ]; then
        "$@" || echo "Note: command failed but installation is continuing: $*"
    else
        sudo "$@" || echo "Note: command failed but installation is continuing: sudo $*"
    fi
}

write_file() {
    local source="$1"
    local destination="$2"

    if [ "$DRY_RUN" -eq 1 ]; then
        echo "[dry-run] write $destination"
        sed 's/^/    /' "$source"
    else
        sudo_run install -m 0644 "$source" "$destination"
    fi
}

require_command() {
    local command_name="$1"
    if ! command -v "$command_name" >/dev/null 2>&1; then
        echo "ERROR: required command not found: $command_name" >&2
        exit 1
    fi
}

uid_for_user() {
    local user_name="$1"
    id -u "$user_name" 2>/dev/null || true
}

gid_for_group() {
    local group_name="$1"
    local gid=""

    if command -v getent >/dev/null 2>&1; then
        gid="$(getent group "$group_name" | awk -F: '{print $3}')"
    fi

    if [ -z "$gid" ] && command -v dscl >/dev/null 2>&1; then
        gid="$(dscl . -read "/Groups/$group_name" PrimaryGroupID 2>/dev/null | awk '{print $2}')"
    fi

    if [ -z "$gid" ] && command -v dscacheutil >/dev/null 2>&1; then
        gid="$(dscacheutil -q group -a name "$group_name" 2>/dev/null | awk '/^gid: / {print $2; exit}')"
    fi

    if [ -z "$gid" ]; then
        gid="$(id -g "$group_name" 2>/dev/null || true)"
    fi

    printf '%s' "$gid"
}

warn_if_mounty_backend_unreachable() {
    if ! install_mounty || [ "$NO_START" -eq 1 ]; then
        return
    fi

    if ! command -v curl >/dev/null 2>&1; then
        echo "Warning: curl is not available; skipping mounty backend preflight."
        return
    fi

    if curl -fsS "$SERVER_URL/list/" >/dev/null 2>&1; then
        return
    fi

    echo
    echo "Warning: mounty backend preflight failed."
    echo "  mounty will probe: $SERVER_URL/list/"
    echo "  If that endpoint is not reachable, mounty exits during startup."
    echo "  Start filer first, choose install target 'both', or provide a reachable remote URL."
    echo

    if [ "$ASSUME_YES" -ne 1 ]; then
        read -r -p "Continue installing mounty anyway? [y/N]: " continue_without_backend
        case "$continue_without_backend" in
            y|Y|yes|YES) ;;
            *)
                echo "Aborted."
                exit 0
                ;;
        esac
    fi
}

install_filer() {
    [ "$INSTALL_TARGET" = "both" ] || [ "$INSTALL_TARGET" = "filer" ]
}

install_mounty() {
    [ "$INSTALL_TARGET" = "both" ] || [ "$INSTALL_TARGET" = "mounty" ]
}

manager="$(detect_manager)"

case "$manager" in
    systemd)
        DEFAULT_BASE_DIR="/var/lib/rusty-fs/data"
        DEFAULT_MOUNTPOINT="/mnt/rusty-fs"
        DEFAULT_LOG_DIR=""
        ;;
    launchd)
        DEFAULT_BASE_DIR="/usr/local/var/rusty-fs/data"
        DEFAULT_MOUNTPOINT="/Volumes/rusty-fs"
        DEFAULT_LOG_DIR="/usr/local/var/log/rusty-fs"
        ;;
esac

DEFAULT_BIN_DIR="/usr/local/bin"
DEFAULT_PORT="3000"
DEFAULT_SERVER_URL="http://127.0.0.1:${DEFAULT_PORT}"
DEFAULT_RUST_LOG="info"
DEFAULT_CHUNK_SIZE="4194304"
DEFAULT_MAX_BUFFER_SIZE="8388608"
DEFAULT_MOUNTY_USER="${SUDO_USER:-$(id -un)}"
if [ "$DEFAULT_MOUNTY_USER" = "root" ] && [ "$(id -un)" != "root" ]; then
    DEFAULT_MOUNTY_USER="$(id -un)"
fi
DEFAULT_MOUNTY_GROUP="$(id -gn "$DEFAULT_MOUNTY_USER" 2>/dev/null || id -gn)"
DEFAULT_MOUNTY_UID="${SUDO_UID:-$(id -u)}"
DEFAULT_MOUNTY_GID="${SUDO_GID:-$(id -g)}"

echo "rusty-fs interactive installer"
echo "Service manager: $manager"
if [ "$DRY_RUN" -eq 1 ]; then
    echo "Mode: dry-run. No system files will be changed."
fi
echo

prompt_choice INSTALL_TARGET "Install target (both/filer/mounty)" "both"
prompt BIN_DIR "Binary install directory" "$DEFAULT_BIN_DIR"

BASE_DIR="$DEFAULT_BASE_DIR"
FILER_PORT="$DEFAULT_PORT"
SERVER_URL="$DEFAULT_SERVER_URL"
MOUNTPOINT="$DEFAULT_MOUNTPOINT"
CHUNK_SIZE="$DEFAULT_CHUNK_SIZE"
MAX_BUFFER_SIZE="$DEFAULT_MAX_BUFFER_SIZE"
MOUNTY_RUN_USER="$DEFAULT_MOUNTY_USER"
MOUNTY_RUN_GROUP="$DEFAULT_MOUNTY_GROUP"
MOUNTY_UID="$DEFAULT_MOUNTY_UID"
MOUNTY_GID="$DEFAULT_MOUNTY_GID"
MOUNTY_EXPOSE_SAME_OWNER=1

if install_filer; then
    prompt BASE_DIR "filer BASE_DIR" "$DEFAULT_BASE_DIR"
    prompt FILER_PORT "filer port" "$DEFAULT_PORT"
fi

if install_mounty; then
    DEFAULT_SERVER_URL="http://127.0.0.1:${FILER_PORT}"
    prompt SERVER_URL "mounty server URL" "$DEFAULT_SERVER_URL"
    prompt MOUNTPOINT "mounty mountpoint" "$DEFAULT_MOUNTPOINT"
fi

prompt RUST_LOG_VALUE "RUST_LOG" "$DEFAULT_RUST_LOG"

if install_mounty; then
    prompt CHUNK_SIZE "MOUNTY_CHUNK_SIZE" "$DEFAULT_CHUNK_SIZE"
    prompt MAX_BUFFER_SIZE "MOUNTY_MAX_BUFFER_SIZE" "$DEFAULT_MAX_BUFFER_SIZE"
    prompt MOUNTY_RUN_USER "Run mounty service as user" "$DEFAULT_MOUNTY_USER"
    prompt MOUNTY_RUN_GROUP "Run mounty service as group" "$DEFAULT_MOUNTY_GROUP"
    prompt_yes_no MOUNTY_EXPOSE_SAME_OWNER "Expose mounted files as same user/group?" "y"

    if [ "$MOUNTY_EXPOSE_SAME_OWNER" -eq 1 ]; then
        MOUNTY_UID="$(uid_for_user "$MOUNTY_RUN_USER")"
        MOUNTY_GID="$(gid_for_group "$MOUNTY_RUN_GROUP")"
        if [ -z "$MOUNTY_UID" ]; then
            echo "ERROR: could not resolve UID for user: $MOUNTY_RUN_USER" >&2
            exit 1
        fi
        if [ -z "$MOUNTY_GID" ]; then
            echo "ERROR: could not resolve GID for group: $MOUNTY_RUN_GROUP" >&2
            exit 1
        fi
    else
        prompt MOUNTY_UID "MOUNTY_UID exposed by FUSE" "$DEFAULT_MOUNTY_UID"
        prompt MOUNTY_GID "MOUNTY_GID exposed by FUSE" "$DEFAULT_MOUNTY_GID"
    fi
fi

if [ "$manager" = "launchd" ]; then
    prompt LOG_DIR "launchd log directory" "$DEFAULT_LOG_DIR"
else
    LOG_DIR=""
fi

echo
echo "Configuration summary:"
echo "  install target: $INSTALL_TARGET"
echo "  manager:        $manager"
echo "  bin dir:        $BIN_DIR"
if install_filer; then
    echo "  BASE_DIR:       $BASE_DIR"
    echo "  filer port:     $FILER_PORT"
fi
if install_mounty; then
    echo "  server URL:     $SERVER_URL"
    echo "  mountpoint:     $MOUNTPOINT"
fi
echo "  RUST_LOG:       $RUST_LOG_VALUE"
if install_mounty; then
    echo "  chunk size:     $CHUNK_SIZE"
    echo "  max buffer:     $MAX_BUFFER_SIZE"
    echo "  run as user:    $MOUNTY_RUN_USER"
    echo "  run as group:   $MOUNTY_RUN_GROUP"
    echo "  mounty UID:     $MOUNTY_UID"
    echo "  mounty GID:     $MOUNTY_GID"
fi
if [ "$manager" = "launchd" ]; then
    echo "  log dir:        $LOG_DIR"
fi
echo

if [ "$ASSUME_YES" -ne 1 ]; then
    read -r -p "Continue? [y/N]: " confirm
    case "$confirm" in
        y|Y|yes|YES) ;;
        *)
            echo "Aborted."
            exit 0
            ;;
    esac
fi

if [ "$NO_BUILD" -eq 0 ] && [ "$DRY_RUN" -eq 0 ]; then
    require_command cargo
fi

if [ "$manager" = "systemd" ] && [ "$DRY_RUN" -eq 0 ]; then
    require_command systemctl
fi

if [ "$manager" = "launchd" ] && [ "$DRY_RUN" -eq 0 ]; then
    require_command launchctl
fi

if [ "$DRY_RUN" -eq 0 ]; then
    warn_if_mounty_backend_unreachable
fi

WORK_DIR="$(mktemp -d)"

if [ "$NO_BUILD" -eq 0 ]; then
    if install_filer; then
        run cargo build --release --manifest-path "$REPO_ROOT/filer/Cargo.toml"
    fi
    if install_mounty; then
        run cargo build --release --manifest-path "$REPO_ROOT/mounty/Cargo.toml"
    fi
fi

sudo_run install -d "$BIN_DIR"

if install_filer; then
    sudo_run install -m 0755 "$REPO_ROOT/filer/target/release/remote-fs-server" "$BIN_DIR/remote-fs-server"
    sudo_run install -d "$BASE_DIR"
fi

if install_mounty; then
    sudo_run install -m 0755 "$REPO_ROOT/mounty/target/release/mounty" "$BIN_DIR/mounty"
    sudo_run install -d "$MOUNTPOINT"
    sudo_run chown "$MOUNTY_RUN_USER:$MOUNTY_RUN_GROUP" "$MOUNTPOINT"
fi

generate_systemd_files() {
    if install_filer; then
        cat > "$WORK_DIR/filer.service" <<EOF
[Unit]
Description=Rusty FS filer HTTP server
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
Environment=BASE_DIR=$BASE_DIR
Environment=RUST_LOG=$RUST_LOG_VALUE
ExecStart=$BIN_DIR/remote-fs-server --port $FILER_PORT
Restart=on-failure
RestartSec=5
KillSignal=SIGTERM
TimeoutStopSec=30

[Install]
WantedBy=multi-user.target
EOF
    fi

    if install_mounty; then
        local after_line="After=network-online.target"
        if [ "$INSTALL_TARGET" = "both" ]; then
            after_line="After=network-online.target filer.service"
        fi

        cat > "$WORK_DIR/mounty.service" <<EOF
[Unit]
Description=Rusty FS mounty FUSE client
$after_line
Wants=network-online.target

[Service]
Type=simple
User=$MOUNTY_RUN_USER
Group=$MOUNTY_RUN_GROUP
Environment=RUST_LOG=$RUST_LOG_VALUE
Environment=MOUNTY_CHUNK_SIZE=$CHUNK_SIZE
Environment=MOUNTY_MAX_BUFFER_SIZE=$MAX_BUFFER_SIZE
Environment=MOUNTY_UID=$MOUNTY_UID
Environment=MOUNTY_GID=$MOUNTY_GID
ExecStart=$BIN_DIR/mounty $SERVER_URL $MOUNTPOINT
Restart=on-failure
RestartSec=5
KillSignal=SIGTERM
TimeoutStopSec=30
ExecStopPost=-/bin/fusermount3 -u $MOUNTPOINT

[Install]
WantedBy=multi-user.target
EOF
    fi
}

xml_escape() {
    local value="$1"
    value="${value//&/&amp;}"
    value="${value//</&lt;}"
    value="${value//>/&gt;}"
    value="${value//\"/&quot;}"
    value="${value//\'/&apos;}"
    printf '%s' "$value"
}

generate_launchd_files() {
    local bin_dir_xml base_dir_xml port_xml server_url_xml mountpoint_xml rust_log_xml
    local chunk_xml max_buffer_xml log_dir_xml uid_xml gid_xml run_user_xml run_group_xml
    bin_dir_xml="$(xml_escape "$BIN_DIR")"
    base_dir_xml="$(xml_escape "$BASE_DIR")"
    port_xml="$(xml_escape "$FILER_PORT")"
    server_url_xml="$(xml_escape "$SERVER_URL")"
    mountpoint_xml="$(xml_escape "$MOUNTPOINT")"
    rust_log_xml="$(xml_escape "$RUST_LOG_VALUE")"
    chunk_xml="$(xml_escape "$CHUNK_SIZE")"
    max_buffer_xml="$(xml_escape "$MAX_BUFFER_SIZE")"
    log_dir_xml="$(xml_escape "$LOG_DIR")"
    uid_xml="$(xml_escape "$MOUNTY_UID")"
    gid_xml="$(xml_escape "$MOUNTY_GID")"
    run_user_xml="$(xml_escape "$MOUNTY_RUN_USER")"
    run_group_xml="$(xml_escape "$MOUNTY_RUN_GROUP")"

    if install_filer; then
        cat > "$WORK_DIR/com.rusty-fs.filer.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>com.rusty-fs.filer</string>
  <key>ProgramArguments</key>
  <array>
    <string>$bin_dir_xml/remote-fs-server</string>
    <string>--port</string>
    <string>$port_xml</string>
  </array>
  <key>EnvironmentVariables</key>
  <dict>
    <key>BASE_DIR</key>
    <string>$base_dir_xml</string>
    <key>RUST_LOG</key>
    <string>$rust_log_xml</string>
  </dict>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <dict>
    <key>SuccessfulExit</key>
    <false/>
  </dict>
  <key>ExitTimeOut</key>
  <integer>30</integer>
  <key>StandardOutPath</key>
  <string>$log_dir_xml/filer.log</string>
  <key>StandardErrorPath</key>
  <string>$log_dir_xml/filer.err.log</string>
</dict>
</plist>
EOF
    fi

    if install_mounty; then
        cat > "$WORK_DIR/com.rusty-fs.mounty.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>com.rusty-fs.mounty</string>
  <key>UserName</key>
  <string>$run_user_xml</string>
  <key>GroupName</key>
  <string>$run_group_xml</string>
  <key>ProgramArguments</key>
  <array>
    <string>$bin_dir_xml/mounty</string>
    <string>$server_url_xml</string>
    <string>$mountpoint_xml</string>
  </array>
  <key>EnvironmentVariables</key>
  <dict>
    <key>RUST_LOG</key>
    <string>$rust_log_xml</string>
    <key>MOUNTY_CHUNK_SIZE</key>
    <string>$chunk_xml</string>
    <key>MOUNTY_MAX_BUFFER_SIZE</key>
    <string>$max_buffer_xml</string>
    <key>MOUNTY_UID</key>
    <string>$uid_xml</string>
    <key>MOUNTY_GID</key>
    <string>$gid_xml</string>
  </dict>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <dict>
    <key>SuccessfulExit</key>
    <false/>
  </dict>
  <key>ExitTimeOut</key>
  <integer>30</integer>
  <key>StandardOutPath</key>
  <string>$log_dir_xml/mounty.log</string>
  <key>StandardErrorPath</key>
  <string>$log_dir_xml/mounty.err.log</string>
</dict>
</plist>
EOF
    fi
}

install_systemd() {
    generate_systemd_files

    if install_filer; then
        write_file "$WORK_DIR/filer.service" "/etc/systemd/system/filer.service"
    fi
    if install_mounty; then
        write_file "$WORK_DIR/mounty.service" "/etc/systemd/system/mounty.service"
    fi

    sudo_run systemctl daemon-reload

    if [ "$NO_START" -eq 0 ]; then
        if install_filer && install_mounty; then
            sudo_run systemctl enable filer.service
            sudo_run systemctl enable mounty.service
            sudo_run_may_fail systemctl stop mounty.service
            sudo_run systemctl restart filer.service
            sudo_run systemctl restart mounty.service
        elif install_filer; then
            sudo_run systemctl enable filer.service
            sudo_run systemctl restart filer.service
        elif install_mounty; then
            sudo_run systemctl enable mounty.service
            sudo_run systemctl restart mounty.service
        fi
    fi
}

install_launchd() {
    generate_launchd_files
    sudo_run install -d "$LOG_DIR"

    if install_filer; then
        write_file "$WORK_DIR/com.rusty-fs.filer.plist" "/Library/LaunchDaemons/com.rusty-fs.filer.plist"
        sudo_run chown root:wheel "/Library/LaunchDaemons/com.rusty-fs.filer.plist"
        sudo_run chmod 0644 "/Library/LaunchDaemons/com.rusty-fs.filer.plist"
    fi
    if install_mounty; then
        write_file "$WORK_DIR/com.rusty-fs.mounty.plist" "/Library/LaunchDaemons/com.rusty-fs.mounty.plist"
        sudo_run chown root:wheel "/Library/LaunchDaemons/com.rusty-fs.mounty.plist"
        sudo_run chmod 0644 "/Library/LaunchDaemons/com.rusty-fs.mounty.plist"
        sudo_run chown "$MOUNTY_RUN_USER:$MOUNTY_RUN_GROUP" "$LOG_DIR"
    fi

    if [ "$NO_START" -eq 0 ]; then
        if install_filer && install_mounty; then
            sudo_run_may_fail launchctl bootout system "/Library/LaunchDaemons/com.rusty-fs.mounty.plist"
            sudo_run_may_fail launchctl bootout system "/Library/LaunchDaemons/com.rusty-fs.filer.plist"
            sudo_run launchctl bootstrap system "/Library/LaunchDaemons/com.rusty-fs.filer.plist"
            sudo_run launchctl bootstrap system "/Library/LaunchDaemons/com.rusty-fs.mounty.plist"
        elif install_filer; then
            sudo_run_may_fail launchctl bootout system "/Library/LaunchDaemons/com.rusty-fs.filer.plist"
            sudo_run launchctl bootstrap system "/Library/LaunchDaemons/com.rusty-fs.filer.plist"
        elif install_mounty; then
            sudo_run_may_fail launchctl bootout system "/Library/LaunchDaemons/com.rusty-fs.mounty.plist"
            sudo_run launchctl bootstrap system "/Library/LaunchDaemons/com.rusty-fs.mounty.plist"
        fi
    fi
}

case "$manager" in
    systemd) install_systemd ;;
    launchd) install_launchd ;;
esac

print_next_steps() {
    echo
    echo "Next steps:"

    if [ "$manager" = "systemd" ]; then
        if [ "$NO_START" -eq 1 ]; then
            echo "  Start services:"
            if install_filer; then
                echo "    sudo systemctl enable --now filer.service"
            fi
            if install_mounty; then
                echo "    sudo systemctl enable --now mounty.service"
            fi
        else
            echo "  Services were installed and restarted."
        fi

        echo "  Check status:"
        if install_filer && install_mounty; then
            echo "    systemctl status filer.service mounty.service"
            echo "    journalctl -u filer.service -u mounty.service -f"
        elif install_filer; then
            echo "    systemctl status filer.service"
            echo "    journalctl -u filer.service -f"
        elif install_mounty; then
            echo "    systemctl status mounty.service"
            echo "    journalctl -u mounty.service -f"
        fi

        if install_mounty; then
            echo "  Check backend:"
            echo "    curl -fsS '$SERVER_URL/list/'"
            echo "  Check mount:"
            echo "    mount | grep '$MOUNTPOINT'"
        fi

        echo "  Stop services:"
        if install_mounty; then
            echo "    sudo systemctl stop mounty.service"
        fi
        if install_filer; then
            echo "    sudo systemctl stop filer.service"
        fi
    fi

    if [ "$manager" = "launchd" ]; then
        if [ "$NO_START" -eq 1 ]; then
            echo "  Start services:"
            if install_filer; then
                echo "    sudo launchctl bootstrap system /Library/LaunchDaemons/com.rusty-fs.filer.plist"
            fi
            if install_mounty; then
                echo "    sudo launchctl bootstrap system /Library/LaunchDaemons/com.rusty-fs.mounty.plist"
            fi
        else
            echo "  Services were installed and restarted."
            echo "  A bootout warning can be harmless when the service was not already loaded."
        fi

        echo "  Check status:"
        if install_filer; then
            echo "    sudo launchctl print system/com.rusty-fs.filer"
        fi
        if install_mounty; then
            echo "    sudo launchctl print system/com.rusty-fs.mounty"
        fi

        echo "  Follow logs:"
        if install_filer; then
            echo "    tail -f '$LOG_DIR/filer.log' '$LOG_DIR/filer.err.log'"
        fi
        if install_mounty; then
            echo "    tail -f '$LOG_DIR/mounty.log' '$LOG_DIR/mounty.err.log'"
        fi

        if install_mounty; then
            echo "  Check backend:"
            echo "    curl -fsS '$SERVER_URL/list/'"
            echo "  Check mount:"
            echo "    mount | grep '$MOUNTPOINT'"
        fi

        echo "  Stop services:"
        if install_mounty; then
            echo "    sudo launchctl bootout system /Library/LaunchDaemons/com.rusty-fs.mounty.plist"
        fi
        if install_filer; then
            echo "    sudo launchctl bootout system /Library/LaunchDaemons/com.rusty-fs.filer.plist"
        fi
    fi
}

echo
echo "Done."
if [ "$DRY_RUN" -eq 1 ]; then
    echo "Dry-run completed. Re-run without --dry-run to install."
fi
print_next_steps
