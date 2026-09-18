#!/bin/bash
set -euo pipefail

RULE_FILE="/etc/polkit-1/rules.d/49-kvn-tui.rules"
GROUP_NAME="kvn-tui"
STAMP_DIR="/var/lib/kvn/integrations"
RULE_SOURCE="${1:?missing embedded polkit rule source}"

if [[ $EUID -ne 0 ]]; then
    echo "This installer must be run as root (e.g. sudo kvn setup --polkit)" >&2
    exit 1
fi

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
else
    ADDED_TO_GROUP=0
fi

RULE_TMP="$(mktemp)"
trap 'rm -f "$RULE_TMP"' EXIT
printf '%s' "$RULE_SOURCE" >"$RULE_TMP"

install -m 0644 -o root -g root "$RULE_TMP" "$RULE_FILE"
install -dm755 "$STAMP_DIR"
sha256sum "$RULE_FILE" | cut -d' ' -f1 > "$STAMP_DIR/polkit.sha256"
chmod 644 "$STAMP_DIR/polkit.sha256"

echo "Installed $RULE_FILE with three systemd-resolved permissions."
echo "NetworkManager permissions are not granted."
if [[ "$ADDED_TO_GROUP" == "1" ]]; then
    echo "User '$USER_NAME' was added to '$GROUP_NAME'."
    echo "Reboot to activate the kvn-tui group."
fi
