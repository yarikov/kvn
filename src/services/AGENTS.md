# Agent Guide: Background services

This guide extends the root [`AGENTS.md`](../../AGENTS.md), whose rules apply here too. It covers `src/services/` and the integration payloads in `contrib/`: the kill switch, suspend/resume, and the `state.json` written for status bars. Read it before changing that code.

## Suspend / Resume
- `services/suspend.rs` runs a blocking zbus listener in a dedicated thread. On resume (`PrepareForSleep` with `false`), it sends `Msg::SystemResumed` through the `mpsc` channel so `update/connection.rs` can schedule a reconnect effect.

## Kill Switch
- Uses **nftables** + a systemd unit (`kvn-tui-killswitch.service`) that loads `/etc/kvn-tui/killswitch.nft`. The ruleset drops all outbound traffic except localhost, `kvn*` interfaces, and packets marked `0x29a` by sing-box.
- Privilege escalation via **sudoers NOPASSWD** (not polkit) — grants the `kvn-tui` group passwordless access to `/usr/lib/kvn-tui/killswitch-helper.sh`. Installed with `sudo kvn setup --killswitch`.
- **Toggle flow**: `Shift+K` keybinding → `Effect::ApplyKillSwitch { enabled }` → daemon spawns thread calling `services::killswitch::apply(enabled)` → sends `Msg::KillSwitchApplied { enabled, error }` back. On success the boolean is flipped and config is saved; on error the boolean is unchanged and the error is shown. Disabling from the keybinding asks for confirmation first (`src/app/AGENTS.md` § Disable Confirmation).
- **Group check on enable**: `apply(true)` reads `integration_group_status()` (`Active` / `PendingActivation` / `NotMember`). A pending group asks the user to reboot (logging out is not enough: `kvn-tui.service` inherits groups from the long-lived `systemd --user` manager); a missing membership points to `sudo kvn setup --killswitch`. `kvn doctor` reports the same two cases.
- **Reconciliation on startup**: daemon queries systemd to check whether the unit is actually active and aligns `settings.kill_switch` with the real state, preventing drift if the unit was manually disabled or the helper was uninstalled.
- **Outdated files**: the installed ruleset, unit, helper, and sudoers rule come from `integration_files`. `kvn doctor` fails when they differ from the embedded payloads. The sudoers file is unreadable to users, so it is checked through `/var/lib/kvn/integrations/killswitch-sudoers.sha256`; the polkit rule is checked the same way through `polkit.sha256`.
- **Disable without helper**: if the helper is missing and the unit is already inactive (e.g. after `sudo kvn clean --killswitch` with the daemon still running), disabling succeeds without calling the helper and just clears `settings.kill_switch`.
- **No handshake window**: the daemon neither pre-resolves nor allowlists the VPN endpoint. sing-box dials the VPN server and DNS servers through its default dialer, which applies `route.default_mark`, so `meta mark 0x29a` admits them; a profile given by hostname therefore connects even when the system resolver uses a public DNS server the kill switch blocks. The helper's `allow` operation and the `handshake_v4`/`handshake_v6` sets stay for compatibility with a still-running older daemon, which adds entries while it connects, and `revoke` on disconnect still flushes entries an older daemon left behind.
- **Ruleset reloads**: `sudo kvn setup --killswitch` applies a new ruleset with `systemctl reload` (`ExecReload` runs the same `nft -f`), never `restart`, whose `ExecStop` deletes the table and leaves the host unprotected. `contrib/killswitch.nft` flushes its chains instead of deleting the table, because recreating base chains unhooks them for a moment even inside one transaction. So a ruleset change may rewrite rules and policies only: changing a chain's hook or priority, a set's type or flags, or removing a chain or set needs an explicit transition that keeps the drop policy hooked, and must be verified by reloading over the previous release's ruleset.
- **sing-box integration**: all sing-box packets carry `default_mark=666` (fwmark `0x29a`); the nftables rule `meta mark 0x29a accept` lets them through. This ensures Bypass/Only geo-routing modes work correctly even with the kill switch active.
- **UI**: the status bar shows a `[KS]` badge when the kill switch is enabled.

## State I/O
- `services/waybar.rs` writes a small JSON file (`state.json`) on every connect/disconnect. It stores connection status, active profile name, and sing-box PID.
- Used by the `--waybar-status` CLI flag and for crash recovery (state is cleared on startup).
