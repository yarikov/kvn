# kvn package migrations

Migrations are shipped here and installed into `/usr/lib/kvn/migrations`. The
runner executes every migration in bytewise filename order.

A migration can ship in any release; it does not have to be a breaking one.
Do not add a bootstrap migration merely to demonstrate the mechanism — the
first script here must do real work for the release that introduces it.

Use a monotonic Unix timestamp and a short slug:

```text
1789000000-refresh-killswitch.sh
```

Every script must start with an interpreter and summary:

```bash
#!/bin/bash
# kvn:summary=Refresh the installed kill-switch integration
# kvn:introduced=0.31.0
set -euo pipefail
```

## What the runner does

1. Takes `$XDG_RUNTIME_DIR/kvn/migrate.lock` so only one runner runs at a time.
2. Waits for any active pacman transaction to finish.
3. Saves `profiles.json` to `~/.config/kvn-tui/recovery/`.
4. Runs the pending queue in order, writing a per-user marker as soon as each
   script succeeds.
5. Hands off to the daemon (below).

The daemon, the VPN, the live config and the kill switch stay up for the whole
run, so scripts have working network. Ordinary migrations must not stop kvn,
sing-box, or the active VPN; the runner owns the daemon handoff.

Each script is its own unit of work. A run that fails stops there, keeps the
markers of the scripts that already succeeded, and the next run resumes from the
one that failed. A script still has to be safe to rerun, because a failure part
way through its own body replays that script.

So: scripts must be idempotent, must exit successfully when they do not apply,
and must call `sudo` only for the exact privileged commands they require. A
machine-wide operation must use `/var/lib/kvn/migrations/<migration-id>` as its
own root-owned completion marker, since a per-user marker cannot describe it.

Reinstalling an integration through `kvn clean --*` and then `kvn setup --*` is
not a neutral round-trip: the clean scripts `groupdel` the shared group once the
other integration is gone, so the rebuilt group gets a new GID the user's live
session does not carry, and `clean --killswitch` / `clean --polkit` also tell
the daemon to turn `kill_switch` and `auto_connect` off.

While anything is pending the daemon refuses to start, so the VPN stays down
until the queue has run. Launching `kvn` runs it automatically before the TUI
opens; `kvn migrate` is for re-running it after a failure. An active pacman
transaction does not block daemon startup — nothing is pending until its
payload is installed — but the runner does wait for it.

## Daemon handoff

Once the queue finishes, the running daemon still holds the pre-migration
config and the previous binary. The runner therefore connects to it and either:

- restarts `kvn-tui.service` itself, when no TUI session is attached; or
- sends `RestartRequired`, when one is. The daemon then freezes config
  persistence and background work, and the attached TUI shows a modal overlay
  that cannot be dismissed: `Enter` quits the TUI and restarts the service,
  `Ctrl+C` stops the daemon outright. Either way the next `kvn` launch comes
  up on the new binary, so the overlay never asks the user to type a command.

Only `kvn` closes the loop: `kvn migrate` runs the queue and exits, so anything
a script stopped stays stopped until the next launch.

Sessions are counted from `AttachSession`, which only the TUI sends, so a
one-shot CLI client is never mistaken for an open window. The freeze still
lets `Quit` through: the daemon turns `SIGTERM` into it, and swallowing it
would stall the very restart being asked for.

## Profile schema changes

Migration scripts have nothing to do with the persisted schema. `profiles.json`
migrates itself: `load_config_at` runs the ordered `Config::migrate_to` steps up
to `CURRENT_SCHEMA_VERSION` and writes the result back when the version changed,
as a compare-and-swap against the bytes it read. A config newer than the build
supports is refused instead of migrated.

So a release that changes the schema adds a `Config::migrate_to` step and bumps
`CURRENT_SCHEMA_VERSION`, and ships a package migration only if it also has
system-level work to do. Scripts must not write `profiles.json`.

## Omarchy integration

A script that needs to refresh the `yarikov.omakvn` bar plugin calls the
ordinary installer, which pulls from the plugin's Git remote:

```bash
command -v omarchy >/dev/null || exit 0
kvn setup --omarchy
```

Never remove a released migration while upgrades from the release preceding
it remain supported. This is what makes a direct jump across several breaking
releases equivalent to applying each release in turn.
