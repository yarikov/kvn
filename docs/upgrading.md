# Upgrading kvn

Upgrade `kvn` through your usual package manager:

```bash
yay -Syu
```

Migrations are applied automatically when needed.

`kvn update` is also available as a convenience command for updating the AUR
package and running pending migrations.

Use `kvn migrate --pending` to check pending migrations and `kvn doctor` if an
upgrade needs attention.

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
