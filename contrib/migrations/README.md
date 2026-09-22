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
4. Copies it to a disposable candidate, `.profiles.json.migrating`.
5. Runs the pending queue in order.
6. Brings the candidate up to the current schema.
7. Validates it and replaces the live `profiles.json` with it, once.
8. Writes a per-user marker for every script in the queue.
9. Hands off to the daemon (below).

The daemon, the VPN, the live config and the kill switch stay up for the whole
run, so scripts have working network. Ordinary migrations must not stop kvn,
sing-box, or the active VPN; the runner owns the daemon handoff.

Markers are written only in step 8, once the whole queue and its config result
are durable. A run that fails anywhere replays the **entire** queue next time,
against a candidate rebuilt from the untouched live config — that is what keeps
the scripts and the candidate from drifting apart, and it is why scripts must
be idempotent.

So: scripts must be idempotent, must exit successfully when they do not apply,
and must call `sudo` only for the exact privileged commands they require. A
machine-wide operation must use `/var/lib/kvn/migrations/<migration-id>` as its
own root-owned completion marker, since a per-user marker cannot describe it.

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

Sessions are counted from `AttachSession`, which only the TUI sends, so a
one-shot CLI client is never mistaken for an open window. The freeze still
lets `Quit` through: the daemon turns `SIGTERM` into it, and swallowing it
would stall the very restart being asked for.

## Profile schema changes

Migration scripts must not write the live `profiles.json`. Persisted schema
changes are implemented as ordered `Config::migrate_to` steps. The script for a
release invokes `kvn config migrate --to N`; the runner supplies the **candidate**
path through `KVN_MIGRATION_PROFILES_PATH` and rejects calls made without it.
The variable is unset when there is no config file yet.

Pinning `N` is mandatory: it prevents a direct jump across several releases
from applying later config steps before intervening package scripts.

Because every step edits the candidate, the live file never holds an
intermediate schema version: a queue that fails midway leaves it exactly as it
was. It is replaced once, after the whole queue succeeded and the result loaded
and validated, and that write is a compare-and-swap against the bytes the
runner started from — so a concurrent writer makes the migration fail with the
backup intact instead of clobbering the file.

Ordinary config loading rejects an old schema and never migrates implicitly.

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
