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

if [[ "$REFRESH" == "1" ]]; then
    echo "Updating kvn kill switch files…"
else
    echo "Installing kvn kill switch for user '$USER_NAME'…"
fi

if [[ "$REFRESH" != "1" ]] && ! getent group "$GROUP_NAME" >/dev/null; then
    groupadd --system "$GROUP_NAME"
    echo "Created system group '$GROUP_NAME'."
fi

RULESET_TMP="$(mktemp)"
HELPER_TMP=""
SUDOERS_TMP=""
trap 'rm -f "$RULESET_TMP" "$HELPER_TMP" "$SUDOERS_TMP"' EXIT

# ── 1. nftables ruleset ────────────────────────────────────────────────
printf '%s' "$RULESET_SOURCE" >"$RULESET_TMP"

# Syntax-check before installing the unit so we don't ship a broken ruleset.
if ! nft -c -f "$RULESET_TMP"; then
    echo "FATAL: the kill-switch ruleset failed nft syntax check" >&2
    exit 1
fi
install -dm755 /etc/kvn-tui
install -m 0644 -o root -g root "$RULESET_TMP" /etc/kvn-tui/killswitch.nft

# ── 2. Helper script ───────────────────────────────────────────────────
install -dm755 /usr/lib/kvn-tui
HELPER_TMP="$(mktemp)"
printf '%s' "$HELPER_SOURCE" >"$HELPER_TMP"
bash -n "$HELPER_TMP"
install -m 0755 -o root -g root "$HELPER_TMP" /usr/lib/kvn-tui/killswitch-helper.sh

# ── 3. systemd unit ────────────────────────────────────────────────────
printf '%s' "$UNIT_SOURCE" > /etc/systemd/system/kvn-tui-killswitch.service
chmod 644 /etc/systemd/system/kvn-tui-killswitch.service

# ── 4. sudoers fragment (validated before installing) ──────────────────
SUDOERS_TMP="$(mktemp)"
printf '%s' "$SUDOERS_SOURCE" > "$SUDOERS_TMP"
if ! visudo -cf "$SUDOERS_TMP" >/dev/null; then
    echo "FATAL: sudoers fragment failed validation" >&2
    exit 1
fi
install -m 0440 -o root -g root "$SUDOERS_TMP" /etc/sudoers.d/kvn-tui-killswitch
install -dm755 "$STAMP_DIR"
sha256sum /etc/sudoers.d/kvn-tui-killswitch | cut -d' ' -f1 > "$STAMP_DIR/killswitch-sudoers.sha256"
chmod 644 "$STAMP_DIR/killswitch-sudoers.sha256"

# ── 5. Ensure user is in the dedicated group ──────────────────────────
if [[ "$REFRESH" == "1" ]]; then
    NEW_GROUP=0
elif ! id -nG "$USER_NAME" | tr ' ' '\n' | grep -Fxq "$GROUP_NAME"; then
    echo "Adding user '$USER_NAME' to the '$GROUP_NAME' group…"
    usermod -aG "$GROUP_NAME" "$USER_NAME"
    NEW_GROUP=1
else
    NEW_GROUP=0
fi

# ── 6. Reload systemd ──────────────────────────────────────────────────
systemctl daemon-reload

# If the unit is already active, reload the ruleset so changes to the template
# take effect immediately (e.g. when re-running this installer after an upgrade).
if systemctl is-active --quiet kvn-tui-killswitch.service; then
    echo "kvn-tui-killswitch.service is active — reloading ruleset…"
    systemctl reload kvn-tui-killswitch.service
fi

if [[ "$REFRESH" == "1" ]]; then
    echo "Updated kvn kill switch files."
    exit 0
fi

echo
echo "Kill switch components installed:"
echo "  /etc/kvn-tui/killswitch.nft"
echo "  /usr/lib/kvn-tui/killswitch-helper.sh"
echo "  /etc/systemd/system/kvn-tui-killswitch.service"
echo "  /etc/sudoers.d/kvn-tui-killswitch"
echo
echo "Toggle the kill switch from the TUI with Shift+K."
if [[ "$NEW_GROUP" == "1" ]]; then
    echo "User '$USER_NAME' was added to '$GROUP_NAME'."
    echo "Reboot to activate the kvn-tui group."
fi
