# kvn package migrations

Breaking migrations are shipped here and installed into
`/usr/lib/kvn/migrations`. The runner executes every migration in bytewise
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

The runner first prepares any declared Git resources while the daemon remains
fully usable. Only then does it put the daemon into migration mode and save an exact
`profiles.json` backup and creates a disposable candidate from it. It keeps the
daemon, VPN, live config, and kill switch online until the whole ordered queue
succeeds, then performs one short daemon restart for the atomic config and
binary handoff. Do not split the script queue into migration classes: a later migration
may depend on every earlier migration having completed.

Scripts must be idempotent, exit successfully when they do not apply, and call
`sudo` only for the exact privileged commands they require. A machine-wide
operation must use `/var/lib/kvn/migrations/<migration-id>` as its own
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

## Git resources

Migration scripts must not access the network, including through setup helpers.
Declare downloads beside the script in `<script-stem>.resources.json`, for
example `1789000000-refresh-plugin.resources.json` for
`1789000000-refresh-plugin.sh`:

```json
{
  "when": {
    "omarchy": true,
    "version": ">=4.0.0, <5.0.0"
  },
  "git": [
    {
      "id": "omakvn",
      "url": "https://github.com/yarikov/omakvn.git",
      "commit": "<replace with the full immutable commit SHA>"
    }
  ]
}
```

Manifests are optional, root-owned package payloads with mode `0644`. Resource
IDs may contain ASCII letters, digits, hyphens and underscores. URLs must be
HTTPS without embedded credentials, query parameters or fragments; branches
and tags are not accepted as commits. Submodules and Git LFS are unsupported.
Scripts and manifests are immutable after release.

`when` applies to the entire resource manifest. Set `"omarchy": true` for
Omarchy-only resources and optionally constrain its version with a standard
semver requirement. For example, `">=4.0.0, <5.0.0"` selects Omarchy 4.
Omitting `version` accepts any installed Omarchy version. Before any Git command
or cache creation, the runner checks `omarchy version`; its numeric package
release suffix (for example `-1` in `4.0.3-1`) is ignored for matching. Plain
Arch and versions outside the declared range skip these resources without
needing Git or network access. If the command fails, or its version is invalid
when a version constraint is present, preparation fails before downloading.
Unknown conditions, `"omarchy": false`, and the legacy string `"omarchy_4"`
are rejected. Omitting `when` makes resources unconditional and does not require
an Omarchy probe.

The runner clones and verifies every applicable resource before freezing the daemon or
creating a profile backup. It rechecks the package queue afterwards. Downloads
do not execute repository code, hooks, submodules or user-configured filters.
No resources means no Git dependency and no download. This is a script-author
contract, not an OS-level network sandbox for arbitrary shell scripts.

Scripts with applicable resources receive `KVN_MIGRATION_RESOURCES_DIR`,
pointing to their own resource directory. The variable is unset when resources
are skipped. The condition does not skip the script itself: a plugin-only
migration should exit successfully when its resources do not apply:

```bash
[[ -n ${KVN_MIGRATION_RESOURCES_DIR:-} ]] || exit 0
kvn setup --omarchy --plugin-source "${KVN_MIGRATION_RESOURCES_DIR:?}/omakvn"
```

For scripts with other migration work, guard only the plugin step instead of
exiting the whole script. Skipped plugin work does not prevent later migrations
from running. The transaction retains its original resource selection on retry;
it never re-detects the desktop and starts additional downloads while frozen.

During a migration, `setup --omarchy` refuses to run without `--plugin-source`.
Local installation validates the plugin identity and clean Git checkout,
copies it independently (including `.git` and its origin), and uses the existing
installer rollback on failure. It never invokes remote plugin add/update and
never silently falls back to a command module. A dirty or unrelated installed
Git checkout is not overwritten. Ordinary manual setup still uses Omarchy's
network-backed installer.

Prepared resources and `ready.json` metadata are private per-user cache data at
`$XDG_STATE_HOME/kvn/migration-resources/<migration-id>-<manifest-digest>/`.
Preparation errors are recorded separately in `preparation.json` for `kvn doctor`;
they do not start a migration session or block the TUI. Re-running preparation
reuses valid completed downloads and discards incomplete temporary clones.
An active transaction records its resource references in
`$XDG_STATE_HOME/kvn/migration-session.json` and is serialized by
`$XDG_RUNTIME_DIR/kvn/migrate.lock`:
retry only verifies local resources, never fetches while migration mode is active.
If those resources are missing or changed, restore them before retrying. An
installed package changed mid-transaction requires recovery with the original
package before preparing its replacement queue.

The private transaction journal is authoritative after migration mode begins.
It records the runner version, exact script/resource manifest, completed IDs,
planned workspace paths, readiness, and the device/inode identities used for
cutover recovery. Never derive completion markers from a newly discovered
package queue. A package change before cutover stops recovery until the original
package is restored; a new queue discovered after the old cutover completes is
processed as a separate ordered transaction.

Workspace paths are journaled before either file is created. On retry the runner
finishes only those planned files and verifies their contents. Device/inode pairs
distinguish pre-exchange and post-exchange layouts even when both JSON files have
identical bytes. Any third layout is ambiguous and must stop without replacing
or deleting user data. Journals from older framework builds may use content
inference only when it proves one unambiguous state.

After successful config/daemon/VPN handoff the runner deletes that transaction's
resource directories. On failure it retains them. Interrupted handoff recovery
does not need the checkouts to reconnect and can finish partially completed
cleanup. Profile recovery backups and the independently installed plugin remain.

Never remove a released migration while upgrades from the release preceding
it remain supported. This is what makes a direct jump across several breaking
releases equivalent to applying each release in turn.
