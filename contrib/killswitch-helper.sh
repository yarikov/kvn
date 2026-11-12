#!/bin/bash
# Narrow privileged API used by the kvn daemon through sudoers.
set -euo pipefail

UNIT="kvn-tui-killswitch.service"
RULESET="/etc/kvn-tui/killswitch.nft"
UNIT_FILE="/etc/systemd/system/$UNIT"
SUDOERS="/etc/sudoers.d/kvn-tui-killswitch"

validate_command_args() {
    [[ $# -ge 1 ]] || return 1
    case $1 in
        check|enable|disable) [[ $# -eq 1 ]] ;;
        *) return 1 ;;
    esac
}

check_root_file() {
    local path=$1 expected_mode=$2 owner mode
    read -r owner mode < <(stat -c '%u %a' "$path") || return 1
    [[ $owner == 0 && $mode == "$expected_mode" ]]
}

check_installation() {
    command -v nft >/dev/null
    command -v systemctl >/dev/null
    check_root_file "$0" 755
    check_root_file "$RULESET" 644
    check_root_file "$UNIT_FILE" 644
    check_root_file "$SUDOERS" 440
}

main() {
    if [[ $EUID -ne 0 ]]; then
        echo "kvn kill-switch helper must run as root" >&2
        exit 1
    fi
    if [[ "$(stat -c %u "$0")" != "0" ]]; then
        echo "kvn kill-switch helper is not root-owned; refusing to run" >&2
        exit 1
    fi
    if ! validate_command_args "$@"; then
        echo "usage: $0 {check|enable|disable}" >&2
        exit 2
    fi

    case "$1" in
        check)
            check_installation
            ;;
        enable|disable)
            exec systemctl "$1" --now "$UNIT"
            ;;
    esac
}

if [[ ${BASH_SOURCE[0]} == "$0" ]]; then
    main "$@"
fi
