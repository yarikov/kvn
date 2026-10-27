# Agent Guide: App behavior

This guide extends the root [`AGENTS.md`](../../AGENTS.md), whose rules apply here too. It covers the module layout of `src/app/update/`, the reducers in `src/app/` and the client code that drives them: the first-run tour (`src/onboarding.rs`, `app/update/onboarding.rs`, `tui_client/handler/key/onboarding.rs`), status events, mode and region selection, disable confirmation, auto-connect, and wheel scrolling and pointer focus (`app/scroll*`, `tui_client/handler/{mouse,wheel,scroll,pointer}.rs`). Read it before changing that code.

## Mode and Region Selection
- `settings.geo_routing.mode` (a `String`) is the `id` of the selected mode in `routing.json`; `GeoRouting::mode(routing)` returns it when the file declares it and `global` otherwise. There is no region setting: the mode picker and Settings › Routing (`Space r`: the Mode row, drafted in `Model::routing_settings_draft`, then the rules and `final` of the shown mode, `Model::shown_mode_rules`, read-only rows `j`/`k` select — `Model::routing_page_lines` is the page's lines, with the blank line and the rules heading as unselectable lines that `scroll::routing_settings_rows` gives the wheel; `Model::routing_settings_selected` clamps the selection to the rows the shown mode has, since a reload or an IPC `SetRoutingMode` can shorten them, and `Model::routing_settings_row` maps it to the Mode row, a rule or `final`; `h`/`l` act only on the Mode row; `Alt+j`/`Alt+k` on a rule row, `update/key/reorder.rs::move_selected_rule`, swap it with its neighbour in `RoutingSettingsDraft::rule_orders`, a `RuleOrderDraft` per mode id holding the order it was moved from (`saved`) and the moved one, which `shown_mode_rules` shows and which is dropped when it is back to `saved`; daemon `Enter` commits only the drafted Mode and keeps the rule orders, which the client saves itself — see `src/tui_client/AGENTS.md`; a reload drops the orders of modes the file no longer has, a failed one drops them all, and `handle_routing_edit_saved`, every routing editor save, drops them all) list `RoutingConfig::mode_ids` in file order, and what is downloaded is the rule-sets of every mode in the file (`route_plan::selectable_rule_sets`).
- `GeoRegion` survives only to add default modes: the tour's region picker commits through `commit_geo_region`, which calls `RoutingConfig::add_region_modes` on `Model::routing` (pure; it adds the missing `bypass_<region>` / `only_<region>` and their rule-sets, and refuses a change that would make the file invalid) and emits `Effect::AddRegionModes`. The daemon repeats the change on disk with `routing::update_routing_at` and answers `Msg::RegionModesSaved`, which applies the file as written or reports that it was not saved. A region whose modes exist, and `global`, change nothing. The selected mode does not change: the `Routing` card follows.
- The first-run tour (see § First-Run Onboarding) owns the region choice: its `Region` card hands off to the region picker, and the mode and region pickers open from nowhere else. Both end only by choosing — they ignore `q`/`Esc` and offer no close action. A mode set over IPC while the mode picker is open leaves it on screen with that mode marked active, and `Enter` confirms or replaces it; the region picker marks nothing active. IPC `SetGeoRegion` is accepted and ignored.
- How a v7 region becomes the selected mode is in `src/config/AGENTS.md`, bullet **Schema v7 → v8**.

## First-Run Onboarding

- On a brand-new install `kvn` opens a guided tour instead of the bare region
  picker. Cards in order: `Welcome`, `Region`, `Routing`, `Profiles`,
  `Connected`, `Doctor`, `Omarchy`, `AutoConnect`, `KillSwitch`, `Finish`
  (`onboarding::OnboardingStep::ORDER`). `Connected` acknowledges the first
  working tunnel and separates the "get it running" half from the "tune it"
  half. `Omarchy` precedes the two protection cards on purpose:
  `kvn setup --omarchy` clones the plugin from GitHub, and a kill switch enabled
  one card earlier leaves the machine without internet until the reboot its
  pending group membership needs — the user could not finish that step, nor turn
  the kill switch back off before rebooting.
- **`Omarchy` is conditional**, so the tour is ten cards only on an Omarchy
  desktop that does not have the bar plugin yet, and nine otherwise.
  `OnboardingStep::sequence(include_omarchy_card)` filters `ORDER`, and `index` / `total` /
  `next` all read from it, so the `4/10` counter, the advance order and what is
  rendered cannot disagree. The flag is `OnboardingProgress::include_omarchy_card`
  — it says whether the card is in the sequence, not whether Omarchy is present,
  and an installed plugin makes it `false` — decided
  once per process by `model::show_omarchy_card()` —
  `omarchy::detect_omarchy_theme().is_some() && !omarchy::omakvn_plugin_installed()`,
  two cheap file reads of the kind `theme_slugs` already does. It is an
  environment fact and never persisted, but it *is* sent over IPC as
  `StateSnapshot::onboarding_omarchy`: the card's own command installs the plugin
  that removes the card, so a client started after `kvn setup --omarchy` would
  otherwise count a shorter tour than the daemon and render `Omarchy` with a
  nonsense counter. The daemon's answer wins; `Model::from_config`'s own guess is
  only the pre-attach default. `OnboardingProgress::new` normalises a step
  recorded as `Omarchy` but resumed without that card.
- Progress lives in `$XDG_STATE_HOME/kvn/onboarding.json` as
  `OnboardingState { step, completed_at }` — deliberately outside the versioned
  `profiles.json` schema, like the support prompt. `Model.onboarding` wraps it in
  `OnboardingProgress`, adding the session-local `awaiting` (a handoff is in
  flight) and `card_pending` (a result arrived; show the card when the screen is
  free). The client reads the active card from `Overlay::Onboarding(step)`;
  `awaiting` is carried separately as `StateSnapshot::onboarding_awaiting`
  because the pickers render differently while the tour holds them.
  `StateSnapshot::onboarding_complete` carries `is_complete()` for the Omarchy
  plugin, which blocks its controls until the tour is finished: neither the
  overlay nor `awaiting` covers the `card_pending` gap. The TUI ignores it.
- An install whose `profiles.json` still names a geo region (schema v7 and
  earlier, `GeoRouting::predates_mode_selection`) is **grandfathered**:
  `onboarding::load_for_daemon` records it complete and persists that, so nobody
  who already uses kvn sees the tour.
- **Card keys**: `Enter` performs the step and moves on; there is no skip, no way
  back and no early exit, so every step is taken exactly once. The card is modal:
  `q`/`Esc` and every other key fall through to a no-op. The two protection
  cards additionally answer their own toggle, `Shift+A` on `AutoConnect` and
  `Shift+K` on `KillSwitch`, which is what their copy tells the user to press. They call `set_auto_connect` / `set_kill_switch`
  rather than the main screen's `toggle_*` helpers: `toggle_*` opens
  `Overlay::ConfirmDisable` when turning a protection off, which would replace
  the card and then close to `Overlay::None`, stranding the tour. A card about
  one setting is explicit enough to skip that dialog, as `Space c` already is.
  A card that shows a shell command answers `y`, which copies it, and the
  `Profiles` card answers `p`, which imports from the clipboard exactly as the
  Profiles list does. Both are handled client-side in
  `tui_client/handler/key/onboarding.rs` because the clipboard is. The card
  renders `OnboardingStep::command(model.integration_setup)`, which spells a
  setup command exactly as the README does — with the `pacman -S --needed`
  install its package needs, over two lines on a `\` continuation, since a paste
  has to work on a machine that has neither `polkit` nor `nftables`. `y` copies
  `clipboard_command`, the same string folded back into one `&&` line: a `\`
  continuation survives a shell but not every terminal's bracketed paste. The
  copy is derived from what is rendered, so the two cannot diverge, and
  `command` gates both, so a card with no command offers no `y` in its footer.
  The card renders it through `Passage::Command`, which emits one line per
  source line instead of refilling it as prose like the surrounding copy. A card only
  answers a key its own copy tells the user to press: the `Profiles` card asks
  for `p`, so importing must not require dismissing the card first. Unlike `copy_selected`, a failed write is reported through
  `IpcCommand::ClientError`: the command is the step's instruction, and silently
  copying nothing is worse than saying so. The last card answers
  `Space` by opening `Overlay::SettingsMenu(Root)`, since that is what its copy
  tells the user to press; it sets `card_pending` rather than finishing, so the
  card returns once that screen is closed and `Enter` still owns completion.
  `?` works on every card through the intercept in `handle_key`, which restores
  the card via `HelpContext::Onboarding`. `?` shows the ordinary help; the tour has no section of its own. A
  daemon restart mid-tour resumes it on the persisted step via
  `IpcCommand::CheckOnboarding`, which `update/onboarding.rs::resume` answers.
- **The protection cards follow their setup state.** `Shift+A` and `Shift+K` are
  useless until `sudo kvn setup --polkit` / `--killswitch` has run *and* the
  `kvn-tui` group both of them gate on is active. So each card renders its tail
  from `Model::integration_setup`: `SetupState::Missing` → `First, set up …` +
  the copyable command, closing with `Reboot once, then press …` or, when
  `group_active` is already true, just `Then press …` — a session that is already
  in the group needs no reboot, because the sudoers and polkit rules both key on
  it. `PendingReboot` → `… setup is complete. Reboot once to activate it, then
  press …`; `Ready` → just `Press …`. `group_active` lives on `IntegrationSetup`
  rather than in `SetupState` because it is one property of the session shared by
  both cards.
  `doctor::integration_setup` produces the whole struct from the existing signals
  (`polkit_status` + `integration_files::polkit_rule_state` for polkit, the
  installed helper + `outdated_killswitch_files` + `killswitch::integration_group_status`
  for the kill switch and for `group_active`, `omarchy::omakvn_plugin_installed`
  for the Omarchy card), with the pure `polkit_setup_state_from` /
  `killswitch_setup_state_from` mappings owning the rules: an unverifiable polkit
  answer reads as `Missing`, because re-running a setup command is harmless while
  claiming it is done is not, and a polkit denial reads as `PendingReboot` only
  while the group itself is pending activation — with the group already active
  the rule is simply not effective, and a user who is not a member (an install
  done by someone else) has to be added by `setup --polkit`, so both keep the
  setup command instead of being sent to a reboot that cannot help. The state
  reaches clients as `StateSnapshot::integration_setup`.
- **Those cards are re-probed while they are on screen**, because the card tells
  the user to run a command in another terminal and then has to show the result.
  `Effect::CheckIntegrationSetup` → `daemon::connection::check_integration_setup`
  (a thread, like the polkit check) → `Msg::IntegrationSetupChecked`, reduced by
  `update/onboarding.rs::integration_setup_checked`, which stores it and returns
  `Effect::BroadcastState` **only when the value changed** so the poll does not
  push a snapshot per tick. Three triggers: `daemon::probe_integration_setup`
  synchronously at startup while the tour is unfinished (so a card's first paint
  is already right), and `app::update::onboarding::probe_visible_card` (not `src/onboarding.rs`) — called by `advance` /
  `resume` / `open_when_idle` right after they set `Overlay::Onboarding`, and by
  the 250 ms tick. That one function owns both the rule (only `Omarchy`,
  `AutoConnect` and `KillSwitch` report an integration) and the gate: the 2 s
  cadence in `Model::last_integration_check_at`, in the same shape as the
  Clash-API sampler because the probe spawns `pkcheck` / `id`, plus
  `Model::integration_check_pending`, which blocks a second probe while one is
  still out — a probe slower than the interval would otherwise be started twice
  and the two answers could land out of order, restoring a stale state on the
  card. `integration_setup_checked` clears the flag before it compares. Keeping
  both callers on one gate is the point: a card open that did not record the
  timestamp let the very next tick fire a second probe. Ordinary daemons never
  pay for any of it: outside the tour no card is open and the startup probe is
  skipped.
  `doctor::integration_setup` resolves the `kvn-tui` group once per probe and
  threads it into `polkit_status_for` as well as both classifiers, so one `id`
  answer serves the whole struct and the two integrations cannot disagree about
  the session; the classifiers themselves stay separate, since their readiness
  criteria differ.
- **The `Omarchy` card has two texts.** Without the plugin it shows
  `kvn setup --omarchy`; with it (installed during the tour — the card itself
  cannot be dropped mid-process, see above) it confirms `kvn is integrated with
  your Omarchy desktop: the widget is on your bar.` and offers no command, so
  `OnboardingStep::command` returns `None` and the footer loses its `y`.
- **Nothing is marked as already done.** The cards carry no `✓` and no green
  title, and the region and mode pickers pass `active: None` to
  `draw_selection_modal` while the tour awaits them, so no entry is painted with
  `Theme::success()`. In a forward-only tour every card on screen is one the user
  has yet to complete; a "done" marker could only come from state that predates
  the tour, which misleads rather than informs.
- **Handoff and resume.** `update/onboarding.rs` owns six transitions.
  `handoff` opens the real screen through the existing `open_*` helpers (which
  seed the drafts those pages `take()`) and persists nothing, so it cannot fail.
  `advance` and `finish` move the card; `outcome(trigger)` reacts to a real
  screen reporting back; `open_when_idle` runs from the 250 ms tick and shows a
  `card_pending` card only when `model.overlay == Overlay::None`, returning
  `Effect::BroadcastState` so the client actually sees it; `resume` answers
  `IpcCommand::CheckOnboarding`. Triggers are raised at
  each screen's own exit point: `commit_geo_region`, `commit_routing_mode` and
  `Msg::Connected`. Only three steps hand off at all — `Region`, `Routing` and
  `Profiles` — and none of their screens can be abandoned, so there is no
  cancellation path and no `Cancelled` trigger.
- **A TUI attaching mid-tour never cancels a handoff.** `resume` reopens the
  stored card only when nothing is in flight (`awaiting.is_none()`) and the
  screen is free. Clearing `awaiting` instead would strand the tour: a client
  attaching while the first connection is still being made would take the
  `Profiles` step off the wait, and the `Msg::Connected` that follows would then
  find no step to finish. The one case where `resume` does act on a handoff is a
  trigger that has already fired — `tunnel_already_up` — since `Msg::Connected`
  is raised once per connection and cannot repeat. `handoff` checks the same
  thing before opening a screen, so a `Profiles` card reached with the tunnel
  already up (a failed progress write restores it) moves on like an
  informational card instead of waiting for an event that has passed.
- **`Profiles` ends at the first connection, not the first import.**
  `Trigger::TunnelUp` is raised only from `on_connected`; `paste.rs` and the
  subscription result handler deliberately raise nothing. A profile on its own
  proves nothing works, so the step is done once the user has actually
  connected with one — standalone or from a subscription, either way. Since
  there is no way to skip a card, the tour cannot be finished before the app has
  been shown to work once.
- **The region and mode steps cannot be abandoned.** `handle_geo_region` and
  `handle_routing_mode` have no `q`/`Esc` branch, so both steps end only by
  committing, which advances the card.
- **`Routing` hands off only where there is a choice.**
  `OnboardingStep::handoff_screen` takes the `RoutingConfig`: when
  `routing.json` has a mode besides `global` it returns
  `HandoffScreen::RoutingMode` (the mode picker, not the full `Space r` page),
  and otherwise `None`, so `Enter` just moves the card on. The card's title and
  primary action branch the same way, and its text names the first country
  whose default modes the file holds. The
  trigger lives in `commit_routing_mode`, placed after its availability check so
  a mode rejected through IPC cannot advance the tour while confirming the
  current mode still does.
- **Failed progress write.** `Effect::PersistOnboarding` carries the whole
  previous `OnboardingProgress` plus an `OnboardingRecovery`, which names what
  has to happen rather than where the transition came from:
  `RestoreCard` (the transition owned the screen) or `WaitForIdle` (a real screen
  is still in front of the user).
  `config_io::restore_onboarding_after_failure` reverts `state`, clears
  `awaiting` (the handed-off screen is gone and will never report again) and
  either restores the card directly or sets `card_pending`;
  `config_io::onboarding_transition` hands the same value to the `SaveConfig`
  error path. Settings the same update already
  committed are never rolled back. On the `SaveConfig` error path in
  `daemon/config_io.rs`, the revert is gated on `config_io::onboarding_transition`, read
  before the effect list is filtered — an unrelated failed save must not reopen
  a finished tour.
- **Support prompt.** The 7-day clock starts from `completed_at`, armed by
  `persist_onboarding` right after a successful write; `check_prompt` gates on
  `onboarding.is_complete()`. `support_prompt::load_for_daemon` keeps a
  start-time path only as crash recovery between the two writes.
- **First-install defaults.** `Config::for_first_run` (used when
  `profiles.json` does not exist and by `kvn config reset`) sets
  `geo_routing.auto_update` to `Every7d`, picks the icon set for the desktop and,
  under Omarchy, the `omarchy` theme sentinel; a pasted subscription gets
  `Every1d`. Every `Default`/`#[serde(default)]`
  in that chain still means `Off`, so a config that omits the field — or the
  whole `geo_routing` or `settings` section — keeps its current behaviour.

## Status Events

- Every status names a `StatusKey` (`push_status(effects, model, key, status)`, `Model::set_status(key, status)`): the key says which earlier status the new one supersedes. `Connecting to X…`, `Connected to X` and `Connection failed` share `Connection`; a subscription's statuses share `Subscription(id)`; connect warnings take `ConnectionWarning(index)` so they do not replace `Connected`. Pick the key of the subject the status reports on, not of the screen that raised it.
- `Model.status_events` keeps the latest event of each key, oldest first and at most 16, each with the `status_revision` it was given; a status with empty text only removes its key's event. It reaches clients as `StateSnapshot::status_events`, so statuses set between two snapshots are not lost. `status` / `status_is_error` still carry the newest status alone for the omakvn plugin and `kvn status`.
- A TUI shows only the events it receives while attached: attaching or reattaching takes the snapshot's `status_revision` as the baseline and shows nothing older, errors included. Earlier statuses stay in the app log.

## Disable Confirmation

- Turning auto-connect or the kill switch **off from the main-screen keybinding** (`Shift+A`, `Shift+K`) opens `Overlay::ConfirmDisable(DisableTarget)` instead of applying immediately: both are protections a stray key press should not remove. Turning them **on** applies right away.
- `update/key/confirm_disable.rs` owns both halves: `toggle_auto_connect` / `toggle_kill_switch` open the dialog when the setting is on and no apply is in flight, and `handle_confirm_disable` commits on `y`/`Enter` (delegating to the unchanged `set_auto_connect` / `set_kill_switch` reducers) or closes on `n`/`q`/`Esc`.
- Every other path bypasses the dialog and calls the `set_*` reducers directly: the Connection settings screen (`Space c`, already an explicit two-step edit), IPC `SetAutoConnect` / `SetKillSwitch` (the Omarchy plugin, `kvn disable --killswitch`, `sudo kvn clean --polkit/--killswitch`), and startup reconciliation — headless callers must never block on a TUI dialog.

## Auto-Connect
- `settings.auto_connect` (persisted in `profiles.json`) controls whether the app reconnects to the last used profile on startup.
- `settings.last_connected_profile` stores the UUID of the most recently connected profile. It is updated in `update/connection.rs` on `Msg::Connected` and saved via `Effect::PersistConfirmedState`.
- `Model::new()` calls `resolve_startup_state()` to check `auto_connect` + `last_connected_profile`. If both are set and the profile exists, the model starts in `ConnectionState::Connecting` with that profile pre-selected, and sets the `Connection` status `Auto-connecting to {name}…`; it is set before any TUI attaches, so no toast shows it.
- The user can toggle `auto_connect` at runtime with the `Shift+A` keybinding, the Connection settings overlay, or IPC `SetAutoConnect`. Disabling saves immediately. Enabling first emits `Effect::CheckAutoConnectPolkit` (with `Model::auto_connect_pending` set): the daemon runs `doctor::polkit_readiness()` and replies with `Msg::AutoConnectPolkitChecked`. The flag is flipped and saved only when passwordless polkit is set up and the `kvn-tui` group is active in the daemon's session; otherwise auto-connect stays off and the reason is shown as an error toast and written to the app log.
- `sudo kvn clean --polkit` / `--killswitch` connect to the invoking user's daemon (`/run/user/$SUDO_UID/kvn-tui.sock`) and send `SetAutoConnect`/`SetKillSwitch { enabled: false }`, so the daemon saves the config as the user. If the daemon is not running, startup reconciliation handles it: `reconcile_kill_switch_state` for the kill switch, and `reconcile_auto_connect_state` turns auto-connect off (before the first tick connects) when `doctor::polkit_authorization_denied()`.

## Kill Switch and Background Work

- While the kill switch is on, ordinary application traffic cannot leave through the physical interface, so the tick holds back geo update checks and subscription auto-updates until the tunnel is `Connected`; started earlier, they race startup auto-connect and nftables drops them.
- `settings.kill_switch` follows the firewall, not the other way round: only `Msg::KillSwitchApplied` changes it, after the helper succeeds. A config reload keeps the old value and reports an edited one as ignored.

## Mouse Wheel Viewports

- `app/scroll.rs` owns pure viewport and selection movement. Wheel scrolling preserves the selected item until it reaches the first visible row (down) or last visible row (up), skipping nonselectable separators toward the inside. At an exhausted viewport edge the remaining steps move selection. Wrapped log rows share a record index.
- `app/scroll/lists.rs` maps selectable Profiles and dialog rows. The TUI keeps viewport offsets locally; `ScrollViewport` carries the normalized overlay context, offset, visible height and signed step. The daemon returns `scroll_result` only with the matching `response_to`, and rejects a changed context. `handler/scroll.rs` queues wheel events and allows one request in flight. Keyboard input, clicks, resize and overlay changes cancel queued work; late replies do not restore cancelled offsets. Cancellation releases the pending slot immediately. Scroll and focus requests expire after two seconds; uncertain scroll requests are discarded without replay. The TUI handshake requires `supports_viewport_scroll` in the snapshot so an older daemon with the same version is restarted through the existing compatibility path.
- Keyboard navigation retains the overlay viewport after wheel scrolling and shifts it only to keep selection visible. The adjusted offset is saved before drawing; changing overlay context clears it.
- Logs use the same calculation locally, create a cursor on the first wheel event, clear visual ranges, and return to following the tail after 15 idle seconds. Wheel acceleration remains shared across panes and dialogs. Mouse selection drags suppress wheel events. Evicting the selected log record clears its cursor and selection while retaining the wheel viewport.

## Pointer Focus and Log Clicks

- Moving or scrolling the pointer over Profiles or Logs focuses that pane, including its border and empty space. Hover leaves selection and viewport intact, is ignored during log dragging or overlays, and sends `SetMainPaneFocus` only when the pane changes. A stationary pointer does not override keyboard focus.
- A left press on a visible log row selects its record through `LogNavigation::select_at`, preserving the viewport's exact wrapped-row offset. Release keeps that cursor; dragging still copies the selected text. Clicking empty space or borders does not select a record. Clicks refresh the 15-second activity timer.

## Module Layout

- **`update` submodules** (`src/app/update/{status,connection,traffic,config_reload,tick,routing,geo,subscription,paste,onboarding}.rs`): Non-keyboard message handlers: status/download-blocked helpers, connect lifecycle + kill-switch/polkit results, Clash-API traffic sampling, `ConfigReloaded`, the 250 ms tick and its auto-update schedules, routing-mode / geo-region / service-routing commits, geo download results, subscription fetch results, clipboard paste → profile or subscription, and the first-run tour's `handoff` / `advance` / `outcome` / `finish` / `open_when_idle` / `resume` transitions plus its `integration_setup_checked` probe result
- **`update::key`** (`src/app/update/key.rs` + `src/app/update/key/`): Keyboard input: `handle_key` routes by `Model.overlay` to `sources` (whose `Alt+j`/`Alt+k` and `Alt+↓`/`Alt+↑` call `reorder`: a profile swaps with its neighbour in the same group — standalone or one subscription — and a subscription with its neighbour among subscriptions, so standalone profiles always stay on top), `confirm_delete`, `confirm_disable`, `settings_menu`, `regions`, `dns`, `onboarding`; `key/ipc.rs` (+ `ipc/{semantic,support}.rs`) handles `IpcCommand` for non-TUI clients
