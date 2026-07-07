#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat <<'EOF'
Usage: deploy/uninstall.sh [options]

Interactive uninstaller for rusty-fs service-manager integration.

Options:
  --dry-run          Print actions without changing system files.
  --yes              Accept defaults and do not prompt.
  --manager NAME     Force service manager: systemd or launchd.
  --remove-binaries  Remove installed binaries from the selected bin directory.
  --remove-mount     Remove the mountpoint directory if it is empty.
  --remove-logs      Remove launchd log files and log directory if empty.
  --help             Show this help.

The uninstaller never removes filer data directories automatically.
EOF
}

DRY_RUN=0
ASSUME_YES=0
MANAGER=""
REMOVE_BINARIES=0
REMOVE_MOUNT=0
REMOVE_LOGS=0

while [ "$#" -gt 0 ]; do
    case "$1" in
        --dry-run) DRY_RUN=1 ;;
        --yes) ASSUME_YES=1 ;;
        --manager)
            MANAGER="${2:-}"
            if [ -z "$MANAGER" ]; then
                echo "ERROR: --manager requires a value" >&2
                exit 1
            fi
            shift
            ;;
        --remove-binaries) REMOVE_BINARIES=1 ;;
        --remove-mount) REMOVE_MOUNT=1 ;;
        --remove-logs) REMOVE_LOGS=1 ;;
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
        "$@" || true
    else
        sudo "$@" || true
    fi
}

require_command() {
    local command_name="$1"
    if ! command -v "$command_name" >/dev/null 2>&1; then
        echo "ERROR: required command not found: $command_name" >&2
        exit 1
    fi
}

bool_label() {
    if [ "$1" -eq 1 ]; then
        printf 'yes'
    else
        printf 'no'
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
        DEFAULT_MOUNTPOINT="/mnt/rusty-fs"
        DEFAULT_LOG_DIR=""
        ;;
    launchd)
        DEFAULT_MOUNTPOINT="/Volumes/rusty-fs"
        DEFAULT_LOG_DIR="/usr/local/var/log/rusty-fs"
        ;;
esac

DEFAULT_BIN_DIR="/usr/local/bin"

echo "rusty-fs interactive uninstaller"
echo "Service manager: $manager"
if [ "$DRY_RUN" -eq 1 ]; then
    echo "Mode: dry-run. No system files will be changed."
fi
echo

prompt_choice INSTALL_TARGET "Uninstall target (both/filer/mounty)" "both"
prompt BIN_DIR "Binary install directory" "$DEFAULT_BIN_DIR"

MOUNTPOINT="$DEFAULT_MOUNTPOINT"
LOG_DIR="$DEFAULT_LOG_DIR"

if install_mounty; then
    prompt MOUNTPOINT "mounty mountpoint" "$DEFAULT_MOUNTPOINT"
fi

if [ "$manager" = "launchd" ]; then
    prompt LOG_DIR "launchd log directory" "$DEFAULT_LOG_DIR"
fi

if [ "$REMOVE_BINARIES" -eq 0 ]; then
    prompt_yes_no REMOVE_BINARIES "Remove installed binaries?" "n"
fi

if install_mounty && [ "$REMOVE_MOUNT" -eq 0 ]; then
    prompt_yes_no REMOVE_MOUNT "Remove mountpoint directory if empty?" "n"
fi

if [ "$manager" = "launchd" ] && [ "$REMOVE_LOGS" -eq 0 ]; then
    prompt_yes_no REMOVE_LOGS "Remove launchd log files?" "n"
fi

echo
echo "Configuration summary:"
echo "  uninstall target: $INSTALL_TARGET"
echo "  manager:          $manager"
echo "  bin dir:          $BIN_DIR"
if install_mounty; then
    echo "  mountpoint:       $MOUNTPOINT"
fi
if [ "$manager" = "launchd" ]; then
    echo "  log dir:          $LOG_DIR"
fi
echo "  remove binaries:  $(bool_label "$REMOVE_BINARIES")"
if install_mounty; then
    echo "  remove mountpoint:$(bool_label "$REMOVE_MOUNT")"
fi
if [ "$manager" = "launchd" ]; then
    echo "  remove logs:      $(bool_label "$REMOVE_LOGS")"
fi
echo "  remove data dirs: no"
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

if [ "$manager" = "systemd" ] && [ "$DRY_RUN" -eq 0 ]; then
    require_command systemctl
fi

if [ "$manager" = "launchd" ] && [ "$DRY_RUN" -eq 0 ]; then
    require_command launchctl
fi

uninstall_systemd() {
    if install_mounty; then
        sudo_run_may_fail systemctl stop mounty.service
        sudo_run_may_fail systemctl disable mounty.service
        sudo_run rm -f /etc/systemd/system/mounty.service
    fi

    if install_filer; then
        sudo_run_may_fail systemctl stop filer.service
        sudo_run_may_fail systemctl disable filer.service
        sudo_run rm -f /etc/systemd/system/filer.service
    fi

    sudo_run systemctl daemon-reload
}

uninstall_launchd() {
    if install_mounty; then
        sudo_run_may_fail launchctl bootout system /Library/LaunchDaemons/com.rusty-fs.mounty.plist
        sudo_run rm -f /Library/LaunchDaemons/com.rusty-fs.mounty.plist
    fi

    if install_filer; then
        sudo_run_may_fail launchctl bootout system /Library/LaunchDaemons/com.rusty-fs.filer.plist
        sudo_run rm -f /Library/LaunchDaemons/com.rusty-fs.filer.plist
    fi
}

remove_optional_files() {
    if [ "$REMOVE_BINARIES" -eq 1 ]; then
        if install_mounty; then
            sudo_run rm -f "$BIN_DIR/mounty"
        fi
        if install_filer; then
            sudo_run rm -f "$BIN_DIR/remote-fs-server"
        fi
    fi

    if install_mounty && [ "$REMOVE_MOUNT" -eq 1 ]; then
        sudo_run rmdir "$MOUNTPOINT"
    fi

    if [ "$manager" = "launchd" ] && [ "$REMOVE_LOGS" -eq 1 ]; then
        if install_mounty; then
            sudo_run rm -f "$LOG_DIR/mounty.log" "$LOG_DIR/mounty.err.log"
        fi
        if install_filer; then
            sudo_run rm -f "$LOG_DIR/filer.log" "$LOG_DIR/filer.err.log"
        fi
        sudo_run rmdir "$LOG_DIR"
    fi
}

case "$manager" in
    systemd) uninstall_systemd ;;
    launchd) uninstall_launchd ;;
esac

remove_optional_files

echo
echo "Done."
if [ "$DRY_RUN" -eq 1 ]; then
    echo "Dry-run completed. Re-run without --dry-run to uninstall."
fi
