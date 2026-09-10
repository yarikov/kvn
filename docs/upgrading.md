# Upgrading kvn

The recommended update command for the AUR binary package is:

```bash
kvn update
```

It updates `kvn-tui-bin` with `yay` (falling back to `paru`) and then runs the
migration scripts supplied by the newly installed package. Scripts are ordered,
resumable, and retained across releases, so jumping over several breaking
releases applies every intermediate migration in sequence.

A normal `yay -Syu` remains supported. In that case migrations run before the
next interactive `kvn` launch. Git resources declared by the package are cloned
at pinned commits first, while the old daemon and VPN remain usable. If this
preparation fails, no migration session or profile backup is created and the
kill switch is not changed; restore network access and retry `kvn migrate`.
Scripts themselves must use the prepared local resources without downloads.
Omarchy plugin resources use an Omarchy semver condition such as
`{"omarchy": true, "version": ">=4.0.0, <5.0.0"}`: only Omarchy major
version 4 downloads them. Plain Arch and other Omarchy versions skip them.
Before the first script, kvn puts the daemon into
migration mode, records the active profile, stores the exact source under
`~/.config/kvn-tui/recovery/`, and creates a candidate from that backup. The
daemon keeps the existing tunnel, live config, and kill switch active while all
scripts run in global filename order against the candidate. Any attached TUI
shows a blocking overlay throughout this interval; `q`/`Esc` only detach it. After
success, the runner briefly stops the old daemon, atomically promotes the
candidate, starts the new daemon, and reconnects the profile. The former live
file is retained temporarily during this handoff and removed after success;
the durable recovery copy remains under `recovery/`. If a script fails, the
running VPN is left alone and the candidate plus private transaction journal are retained.
Prepared resources are also retained on failure. They are removed from
`$XDG_STATE_HOME/kvn-tui/migration-resources/` after a successful daemon/VPN
handoff; this does not remove an installed plugin or recovery backups.
The journal records the exact migration manifest, runner version, planned
workspace paths, and pre-cutover file identities. Completion markers are made
only for that recorded manifest. If another package changes the queue before
cutover, kvn preserves all artifacts and asks for the package version that
started the transaction instead of replaying system actions. After cutover,
newly installed migrations remain pending and run as a separate transaction.
An attached TUI keeps its blocking migration overlay while the daemon socket is
replaced, reconnects to a compatible daemon, and restarts itself after the
journal is cleared.
Use `kvn migrate` to resume, `kvn migrate --pending` to inspect the queue, and
`kvn doctor` to diagnose it. An old `profiles.json` is detected independently
of the package baseline, so a restored config is migrated even after a fresh
package installation.

The transactional daemon protocol begins with kvn 0.30.0. A daemon from an
older release cannot be frozen safely by the new runner; stop it and follow the
version-specific upgrade guide before running migrations. For the initial move
to 0.30.0, stop the pre-framework daemon and run the profile transaction with
the newly installed binary:

```bash
systemctl --user stop kvn-tui.service
kvn migrate
```

The 0.30.0 package does not contain retroactive scripts for older releases.
Only an existing old `profiles.json` is handled by this bootstrap transaction;
all other pre-0.30 changes remain in the manual guides below.

`kvn migrate` starts the updated daemon after the atomic cutover. From 0.30.0
onward, the protocol version is independent of the application version, so a
compatible old daemon can keep the VPN alive while the new package performs the
migration.

The migration framework does not retroactively execute changes from releases
older than 0.30.0. The historical guides below remain the source of truth when
upgrading an older installation to the 0.30.0 baseline.
When upgrading directly from a pre-0.30 release to a later release, migrations
introduced after the installed version remain pending; they are not marked as
applied by the package hook. Follow the historical guides and stop the
pre-framework daemon first, then run `kvn migrate` for the later releases.

## v0.28.0

Users upgrading from v0.27.1 or earlier must refresh existing polkit and
kill-switch installations and apply the steps described in the
[v0.28.0 migration guide](migrations/v0.28.0.md).

## v0.27.0 on Omarchy 4

Follow the [v0.27.0 migration guide](migrations/v0.27.0.md) to install the
standalone `yarikov.omakvn` bar plugin.

## v0.22.0 on Omarchy

Follow the [v0.22.0 migration guide](migrations/v0.22.0.md) to refresh the
Omarchy 3/4 desktop integration.

## v0.20.0

Follow the [v0.20.0 migration guide](migrations/v0.20.0.md) to move daemon
startup from Hyprland autostart to the systemd user service.
