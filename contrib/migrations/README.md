# kvn package migrations

Breaking migrations are shipped here and installed into
`/usr/lib/kvn-tui/migrations`. The runner executes every migration in bytewise
filename order and records successful runs per user.

The framework starts at v0.30.0. Do not add scripts for older releases, and do
not add a bootstrap migration merely to demonstrate the mechanism. The first
script in this directory must belong to an actual breaking release after
v0.30.0.

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

The runner first puts the daemon into migration mode, then saves an exact
`profiles.json` backup and creates a disposable candidate from it. It keeps the
daemon, VPN, live config, and kill switch online until the whole ordered queue
succeeds, then performs one short daemon restart for the atomic config and
binary handoff. Do not introduce migration classes or phases: a later migration
may depend on every earlier migration having completed.

Scripts must be idempotent, exit successfully when they do not apply, and call
`sudo` only for the exact privileged commands they require. A machine-wide
operation must use `/var/lib/kvn-tui/migrations/<migration-id>` as its own
root-owned completion marker. Ordinary migrations must not stop kvn, sing-box,
or the active VPN; the runner owns the daemon handoff.

Migration scripts must not write the live `profiles.json`. Persisted schema
changes are implemented as ordered `Config::migrate_to` steps. The script for a
release invokes `kvn config migrate --to N`; the runner supplies the candidate
path and rejects calls outside an active transaction. Pinning `N` is mandatory:
it prevents a direct jump across several releases from applying later config
steps before intervening package scripts. Ordinary config loading rejects an
old schema. The runner validates and promotes the candidate only after every
pending script has succeeded.

Never remove a released migration while upgrades from the release preceding
it remain supported. This is what makes a direct jump across several breaking
releases equivalent to applying each release in turn.
