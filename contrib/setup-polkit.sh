#!/bin/bash
set -euo pipefail

RULE_FILE="/etc/polkit-1/rules.d/49-kvn-tui.rules"
GROUP_NAME="kvn-tui"
STAMP_DIR="/var/lib/kvn/integrations"
STAMP_FILE="$STAMP_DIR/polkit.sha256"
RULE_SOURCE="${1:?missing embedded polkit rule source}"
REFRESH="${KVN_SETUP_REFRESH:-0}"

if [[ $EUID -ne 0 ]]; then
    echo "This installer must be run as root (e.g. sudo kvn setup --polkit)" >&2
    exit 1
fi

ADDED_TO_GROUP=0
if [[ "$REFRESH" != "1" ]]; then
    USER_NAME="${SUDO_USER:-}"
    if [[ -z "$USER_NAME" || "$USER_NAME" == "root" ]] || ! id "$USER_NAME" >/dev/null 2>&1; then
        echo "Could not identify a non-root invoking user; run this command via sudo." >&2
        exit 1
    fi

    if ! getent group "$GROUP_NAME" >/dev/null; then
        groupadd --system "$GROUP_NAME"
        echo "Created system group '$GROUP_NAME'."
    fi

    if ! id -nG "$USER_NAME" | tr ' ' '\n' | grep -Fxq "$GROUP_NAME"; then
        usermod -aG "$GROUP_NAME" "$USER_NAME"
        ADDED_TO_GROUP=1
    fi
fi

RULE_TMP="$(mktemp)"
trap 'rm -f "$RULE_TMP"' EXIT
printf '%s' "$RULE_SOURCE" >"$RULE_TMP"

RULE_MESSAGE="Installed $RULE_FILE with four systemd-resolved permissions."
if [[ "$REFRESH" != "1" && -f "$RULE_FILE" && ! -L "$RULE_FILE" ]] &&
    cmp -s -- "$RULE_TMP" "$RULE_FILE" &&
    [[ "$(stat -c '%u:%g:%a' -- "$RULE_FILE")" == "0:0:644" ]]; then
    RULE_MESSAGE="Polkit rule is already up to date."
else
    if [[ "$REFRESH" == "1" || -e "$RULE_FILE" || -L "$RULE_FILE" ]]; then
        RULE_MESSAGE="Updated $RULE_FILE."
    fi
    install -T -m 0644 -o root -g root "$RULE_TMP" "$RULE_FILE"
fi

STAMP="$(sha256sum < "$RULE_FILE" | cut -d' ' -f1)"
if [[ "$REFRESH" == "1" || ! -f "$STAMP_FILE" ]] ||
    ! printf '%s\n' "$STAMP" | cmp -s -- - "$STAMP_FILE" ||
    [[ "$(stat -c '%a' -- "$STAMP_FILE")" != "644" ||
        "$(stat -c '%a' -- "$STAMP_DIR")" != "755" ]]; then
    install -dm755 "$STAMP_DIR"
    printf '%s\n' "$STAMP" > "$STAMP_FILE"
    chmod 644 "$STAMP_FILE"
fi

echo "$RULE_MESSAGE"
if [[ "$ADDED_TO_GROUP" == "1" ]]; then
    echo "User '$USER_NAME' was added to '$GROUP_NAME'."
    echo "Reboot to activate the kvn-tui group."
fi
