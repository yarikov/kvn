#!/bin/bash
set -euo pipefail

PLUGIN_ID="yarikov.omakvn"
PLUGIN_DIR="$HOME/.config/omarchy/plugins/$PLUGIN_ID"
SHELL_CONFIG="$HOME/.config/omarchy/shell.json"
HYPR_BINDINGS="$HOME/.config/hypr/bindings.lua"
HYPR_MAIN="$HOME/.config/hypr/hyprland.lua"
LAUNCHER="$HOME/.local/bin/omarchy-launch-kvn-tui"
DESKTOP_ENTRY="$HOME/.local/share/applications/kvn-tui.desktop"
APP_ICON="$HOME/.local/share/icons/hicolor/scalable/apps/kvn-tui.svg"
changed=0

replace_if_changed() {
  local source="$1" target="$2"
  if cmp -s -- "$source" "$target"; then
    rm -- "$source"
    return 1
  fi
  chmod --reference="$target" "$source"
  mv -f -- "$source" "$target"
}

is_omakvn_checkout() {
  local origin
  [[ -d $PLUGIN_DIR/.git ]] || return 1
  origin=$(git -C "$PLUGIN_DIR" remote get-url origin 2>/dev/null || true)
  case "$origin" in
  https://github.com/yarikov/omakvn | https://github.com/yarikov/omakvn.git | git@github.com:yarikov/omakvn.git)
    return 0
    ;;
  esac
  return 1
}

remove_plugin() {
  [[ -e $PLUGIN_DIR || -L $PLUGIN_DIR ]] || return 0
  if command -v omarchy >/dev/null 2>&1 &&
    omarchy plugin remove "$PLUGIN_ID" --yes >/dev/null 2>&1; then
    echo "Removed the $PLUGIN_ID bar plugin."
    changed=1
    return 0
  fi
  [[ -e $PLUGIN_DIR || -L $PLUGIN_DIR ]] || return 0
  if [[ -L $PLUGIN_DIR ]]; then
    rm -f -- "$PLUGIN_DIR"
  elif is_omakvn_checkout; then
    rm -rf -- "$PLUGIN_DIR"
  else
    echo "Warning: $PLUGIN_DIR is not a $PLUGIN_ID checkout; remove it manually." >&2
    return 0
  fi
  echo "Removed the $PLUGIN_ID bar plugin."
  changed=1
}

remove_bar_entry() {
  [[ -f $SHELL_CONFIG ]] || return 0
  command -v jq >/dev/null 2>&1 || {
    echo "Warning: jq is not installed; remove $PLUGIN_ID from $SHELL_CONFIG manually." >&2
    return 0
  }
  local kvn_entry_defs='
    def entry_id: if type == "object" then (.id // "") else tostring end;
    def kvn_entry: entry_id == "kvn-tui" or entry_id == "kvn.tui" or entry_id == "yarikov.omakvn";
  '
  local status=0
  jq -e "$kvn_entry_defs"'
    (.bar.layout | type) == "object"
    and ([.bar.layout[] | arrays | .[] | select(kvn_entry)] | length > 0)
  ' "$SHELL_CONFIG" >/dev/null || status=$?
  case $status in
  0) ;;
  1) return 0 ;;
  *)
    echo "Error: could not read $SHELL_CONFIG; fix it and rerun the cleanup. Backups were kept." >&2
    exit 1
    ;;
  esac
  local tmp
  tmp=$(mktemp "${SHELL_CONFIG}.tmp.XXXXXX")
  jq "$kvn_entry_defs"'
    if (.bar.layout | type) == "object" then
      .bar.layout |= with_entries(
        if (.value | type) == "array" then .value |= map(select(kvn_entry | not)) else . end
      )
    else . end
  ' "$SHELL_CONFIG" >"$tmp"
  jq -e '.version == 1 and (.bar.layout | type == "object")' "$tmp" >/dev/null || {
    rm -- "$tmp"
    echo "Warning: unexpected layout in $SHELL_CONFIG; remove $PLUGIN_ID manually." >&2
    return 0
  }
  if replace_if_changed "$tmp" "$SHELL_CONFIG"; then
    echo "Removed the kvn widget from $SHELL_CONFIG"
    changed=1
  fi
}

remove_marker_block() {
  local file="$1" marker="$2"
  local begin="-- kvn-tui ${marker}: begin" end="-- kvn-tui ${marker}: end"
  [[ -f $file ]] || return 0
  grep -Fxq -- "$begin" "$file" || return 0
  local tmp
  tmp=$(mktemp "${file}.tmp.XXXXXX")
  if ! awk -v begin="$begin" -v end="$end" '
    $0 == begin { if (skipping) { malformed = 1; exit } skipping = 1; held = 0; next }
    $0 == end { if (!skipping) { malformed = 1; exit } skipping = 0; next }
    skipping { next }
    held { print ""; held = 0 }
    $0 == "" { held = 1; next }
    { print }
    END {
      if (malformed || skipping) exit 2
      if (held) print ""
    }
  ' "$file" >"$tmp"; then
    rm -- "$tmp"
    echo "Warning: $file has unpaired kvn ${marker} markers; left unchanged, remove the kvn ${marker} manually." >&2
    return 0
  fi
  if replace_if_changed "$tmp" "$file"; then
    echo "Removed the kvn ${marker} from $file"
    changed=1
  fi
}

warn_about_unmanaged_binding() {
  [[ -f $HYPR_BINDINGS ]] || return 0
  if grep -Fq "omarchy-launch-kvn-tui" "$HYPR_BINDINGS"; then
    echo "Warning: $HYPR_BINDINGS still references omarchy-launch-kvn-tui; remove it manually." >&2
  fi
}

remove_launcher_files() {
  local file
  for file in "$LAUNCHER" "$DESKTOP_ENTRY" "$APP_ICON"; do
    if [[ -e $file || -L $file ]]; then
      rm -f -- "$file"
      echo "Removed $file"
      changed=1
    fi
  done
}

remove_plugin
remove_bar_entry
remove_marker_block "$HYPR_BINDINGS" "keybinding"
remove_marker_block "$HYPR_MAIN" "window rule"
warn_about_unmanaged_binding
remove_launcher_files

if (( ! changed )); then
  echo "No kvn Omarchy integration found."
  exit 0
fi
if command -v hyprctl >/dev/null 2>&1; then
  timeout 15 hyprctl reload >/dev/null 2>&1 || true
fi
if command -v omarchy-shell >/dev/null 2>&1; then
  timeout 15 omarchy-shell shell rescanPlugins >/dev/null 2>&1 || true
fi
