# Agent Guide: Daemon

This guide extends the root [`AGENTS.md`](../../AGENTS.md), whose rules apply here too. It covers `src/daemon/`: how effects are executed and where each kind of side effect lives. Read it before changing that code.

## Module Layout

- **`daemon` submodules** (`src/daemon/{effect,connection,geo,config_io,subscription,traffic,profile_test,process_slot}.rs`): Effect execution, mirroring the `app/update/` handler split: `effect.rs` is the `execute_daemon_effect` dispatcher only; `connection.rs` owns connect/disconnect, the polkit check and the tour's integration probe; `geo.rs` the seven geo/service rule-set effects plus the shared refresh and result-finalizing helpers; `config_io.rs` the revision-checked `profiles.json` commit, the support-prompt and onboarding-progress writes (including the failed-write recovery in `src/app/AGENTS.md` § First-Run Onboarding) and config reload; `subscription.rs`, `traffic.rs` and `profile_test.rs` one effect each (the last owns the temporary sing-box SOCKS5 latency probe); `process_slot.rs` the sing-box process slot, its poisoned-lock-safe accessors, the 250 ms ticker and exit polling

## Config Saves

`config_io::persist_config_changes` commits `profiles.json` after each `update`, before any effect runs. The update chooses how a failed write is undone:

- **`Effect::SaveConfig`** — a change the user asked for. A failed write restores the config, support prompt and tour progress from before the update, preserves the edited version under `recovery/`, and drops every effect except `BroadcastState` and `AppendAppLog`: the change did not happen.
- **`Effect::PersistConfirmedState`** — a record of something the system already did (`KillSwitchApplied`, `Connected`). A failed write keeps the model and every other effect, such as `WriteState`, and only reports the error: the model must describe the firewall and the tunnel as they are. The config the write was based on is kept in `Model::unsaved_state_merge_base` and used as the merge base of every later save, so the unsaved state reads as the model's own change and wins over the older value on disk. A successful write or a reload from disk clears it.
