#!/bin/bash
# Install the kvn-tui kill switch:
#   - /etc/kvn-tui/killswitch.nft           (nftables ruleset)
#   - /usr/lib/kvn-tui/killswitch-helper.sh (privileged helper)
#   - /etc/systemd/system/kvn-tui-killswitch.service
#   - /etc/sudoers.d/kvn-tui-killswitch     (NOPASSWD for group `kvn-tui`)
#
# After install, members of the `kvn-tui` group can toggle the kill switch
# from kvn without a password prompt.
set -euo pipefail

if [[ $EUID -ne 0 ]]; then
    echo "This installer must be run as root (e.g. sudo kvn setup --killswitch)" >&2
    exit 1
fi

REFRESH="${KVN_SETUP_REFRESH:-0}"
USER_NAME="${SUDO_USER:-}"
if [[ "$REFRESH" != "1" ]] &&
    { [[ -z "$USER_NAME" || "$USER_NAME" == "root" ]] || ! id "$USER_NAME" >/dev/null 2>&1; }; then
    echo "Could not identify a non-root invoking user; run this command via sudo." >&2
    exit 1
fi
HELPER_SOURCE="${1:?missing embedded kill-switch helper source}"
RULESET_SOURCE="${2:?missing embedded kill-switch ruleset source}"
UNIT_SOURCE="${3:?missing embedded kill-switch unit source}"
SUDOERS_SOURCE="${4:?missing embedded kill-switch sudoers source}"
GROUP_NAME="kvn-tui"
STAMP_DIR="/var/lib/kvn/integrations"
RULESET_FILE="/etc/kvn-tui/killswitch.nft"
HELPER_FILE="/usr/lib/kvn-tui/killswitch-helper.sh"
UNIT_NAME="kvn-tui-killswitch.service"
UNIT_FILE="/etc/systemd/system/$UNIT_NAME"
SUDOERS_FILE="/etc/sudoers.d/kvn-tui-killswitch"
STAMP_FILE="$STAMP_DIR/killswitch-sudoers.sha256"
STAMP_SOURCE="$(printf '%s' "$SUDOERS_SOURCE" | sha256sum | cut -d' ' -f1)"$'\n'

file_matches() {
    local path="$1" mode="$2" contents="$3"
    [[ -f "$path" && ! -L "$path" ]] &&
        [[ "$(stat -c '%u:%g:%a' -- "$path")" == "0:0:$mode" ]] &&
        printf '%s' "$contents" | cmp -s -- - "$path"
}

prepare_staging_dir() {
    if [[ -z "$TMP_DIR" ]]; then
        TMP_DIR="$(mktemp -d)"
        trap 'rm -rf -- "$TMP_DIR"' EXIT
    fi
}

stage_component() {
    local path="$1" mode="$2" contents="$3" staged
    if [[ -e "$path" || -L "$path" ]]; then
        HAS_INSTALLED_COMPONENTS=1
    fi
    if [[ "$REFRESH" != "1" ]] && file_matches "$path" "$mode" "$contents"; then
        return
    fi

    prepare_staging_dir
    staged="$TMP_DIR/${path##*/}"
    printf '%s' "$contents" > "$staged"
    case "$path" in
        "$RULESET_FILE")
            # Syntax-check before installing the unit so we don't ship a broken ruleset.
            if ! nft -c -f "$staged"; then
                echo "FATAL: the kill-switch ruleset failed nft syntax check" >&2
                exit 1
            fi
            RELOAD_SYSTEMD=1
            ;;
        "$HELPER_FILE")
            bash -n "$staged"
            ;;
        "$UNIT_FILE")
            RELOAD_SYSTEMD=1
            ;;
        "$SUDOERS_FILE")
            if ! visudo -cf "$staged" >/dev/null; then
                echo "FATAL: sudoers fragment failed validation" >&2
                exit 1
            fi
            ;;
    esac
    STAGED_FILES+=("$path")
    STAGED_MODES["$path"]="$mode"
}

repair_directory_mode() {
    local directory="$1"
    if [[ ! -d "$directory" || "$(stat -Lc '%a' -- "$directory")" != "755" ]]; then
        install -dm755 -- "$directory"
        REPAIRED_METADATA=1
    fi
}

REPAIRED_METADATA=0
repair_directory_mode "${STAMP_DIR%/*}"
repair_directory_mode "$STAMP_DIR"
exec {STAMP_DIR_LOCK}<"$STAMP_DIR"
flock -x "$STAMP_DIR_LOCK"

TMP_DIR=""
STAGED_FILES=()
declare -A STAGED_MODES=()
HAS_INSTALLED_COMPONENTS=0
RELOAD_SYSTEMD=0
if [[ ! -f "$STAMP_FILE" ]]; then
    RELOAD_SYSTEMD=1
fi

stage_component "$RULESET_FILE" 644 "$RULESET_SOURCE"
stage_component "$HELPER_FILE" 755 "$HELPER_SOURCE"
stage_component "$UNIT_FILE" 644 "$UNIT_SOURCE"
stage_component "$SUDOERS_FILE" 440 "$SUDOERS_SOURCE"

if (( ${#STAGED_FILES[@]} > 0 )); then
    if [[ "$REFRESH" == "1" || "$HAS_INSTALLED_COMPONENTS" == "1" ]]; then
        FILE_ACTION=updated
        echo "Updating kvn kill switch files…"
    else
        FILE_ACTION=installed
        echo "Installing kvn kill switch for user '$USER_NAME'…"
    fi
fi

if [[ "$REFRESH" != "1" ]] && ! getent group "$GROUP_NAME" >/dev/null; then
    groupadd --system "$GROUP_NAME"
    echo "Created system group '$GROUP_NAME'."
fi

if (( RELOAD_SYSTEMD )); then
    rm -f -- "$STAMP_FILE"
fi

repair_directory_mode "${RULESET_FILE%/*}"
repair_directory_mode "${HELPER_FILE%/*}"
for path in "${STAGED_FILES[@]}"; do
    install -T -m "${STAGED_MODES[$path]}" -o root -g root -- "$TMP_DIR/${path##*/}" "$path"
done

# ── 5. Ensure user is in the dedicated group ──────────────────────────
NEW_GROUP=0
if [[ "$REFRESH" != "1" ]] && ! id -nG "$USER_NAME" | tr ' ' '\n' | grep -Fxq "$GROUP_NAME"; then
    echo "Adding user '$USER_NAME' to the '$GROUP_NAME' group…"
    usermod -aG "$GROUP_NAME" "$USER_NAME"
    NEW_GROUP=1
fi

# ── 6. Reload systemd ──────────────────────────────────────────────────
if (( RELOAD_SYSTEMD )); then
    systemctl daemon-reload

    # If the unit is already active, reload the ruleset so changes to the template
    # take effect immediately (e.g. when re-running this installer after an upgrade).
    if systemctl is-active --quiet "$UNIT_NAME"; then
        echo "$UNIT_NAME is active — reloading ruleset…"
        systemctl reload "$UNIT_NAME"
    else
        status=$?
        if (( status != 3 )); then
            echo "Failed to query $UNIT_NAME (systemctl is-active exited with $status)." >&2
            exit "$status"
        fi
    fi
fi

if ! file_matches "$STAMP_FILE" 644 "$STAMP_SOURCE"; then
    prepare_staging_dir
    printf '%s' "$STAMP_SOURCE" > "$TMP_DIR/stamp"
    install -T -m 0644 -o root -g root -- "$TMP_DIR/stamp" "$STAMP_FILE"
    REPAIRED_METADATA=1
fi

if [[ "$REFRESH" == "1" ]]; then
    echo "Updated kvn kill switch files."
    exit 0
fi

if (( ${#STAGED_FILES[@]} > 0 )); then
    echo
    echo "Kill switch components $FILE_ACTION:"
    printf '  %s\n' "${STAGED_FILES[@]}"
    if [[ "$FILE_ACTION" == installed ]]; then
        echo
        echo "Toggle the kill switch from the TUI with Shift+K."
    fi
elif (( REPAIRED_METADATA )); then
    echo "Kill switch installation metadata repaired."
else
    echo "Kill switch files are already up to date."
fi
if [[ "$NEW_GROUP" == "1" ]]; then
    echo "User '$USER_NAME' was added to '$GROUP_NAME'."
    echo "Reboot to activate the kvn-tui group."
fi
