#!/bin/sh
# Installs or removes Nox's closed-lid sudoers rule. Must run as root:
#   sudo scripts/install-lid-rule.sh install <uid> <remove-legacy: 0|1>
#   sudo scripts/install-lid-rule.sh remove
# Nox runs the same script (embedded) from its administrator prompt.
#
# The rule text is generated here from a numeric uid. Nothing a user process
# wrote is read by root, so there is no temporary file to swap. It lets the
# uid run exactly `pmset -a disablesleep 1` and `pmset -a disablesleep 0` as
# root, without a password. See docs/SLEEP_RECOVERY.md.
#
# The file name contains no dot on purpose: sudo skips files of /etc/sudoers.d
# whose name contains a '.'.
set -u
PATH=/usr/bin:/bin:/usr/sbin:/sbin
export PATH

RULE=/etc/sudoers.d/nox-lid-sleep
LEGACY_LABEL=io.github.martingaltier.nox.helper
LEGACY_PLIST=/Library/LaunchDaemons/$LEGACY_LABEL.plist
LEGACY_BIN=/Library/PrivilegedHelperTools/$LEGACY_LABEL
LEGACY_SOCK=/var/run/$LEGACY_LABEL.sock
LEGACY_STATE="/Library/Application Support/Nox"
LEGACY_JOURNAL="$LEGACY_STATE/helper-state.json"

[ "$(id -u)" = 0 ] || { echo "install-lid-rule.sh: must run as root" >&2; exit 77; }

# Removes the previous privileged helper (daemon), if present.
remove_legacy() {
  # SIGTERM: a running old helper restores lid sleep before exiting.
  launchctl bootout "system/$LEGACY_LABEL" 2>/dev/null || true
  status=0
  if [ -f "$LEGACY_JOURNAL" ] && [ -x "$LEGACY_BIN" ]; then
    "$LEGACY_BIN" --recover || status=$?
  fi
  rm -f "$LEGACY_PLIST" "$LEGACY_BIN" "$LEGACY_SOCK"
  if [ -f "$LEGACY_JOURNAL" ]; then
    echo "install-lid-rule.sh: lid sleep may still be disabled, check with" >&2
    echo "  pmset -g | grep SleepDisabled" >&2
    rm -f "$LEGACY_JOURNAL"
    status=1
  fi
  rmdir "$LEGACY_STATE" 2>/dev/null || true
  return "$status"
}

install_rule() {
  uid=$1
  legacy=$2
  case "$uid" in
    ''|*[!0-9]*|0) echo "install-lid-rule.sh: invalid uid" >&2; exit 65 ;;
  esac
  tmp=$(mktemp /etc/sudoers.d/.nox-lid-sleep.XXXXXX) || exit 73
  trap 'rm -f "$tmp"' EXIT
  printf '#%s ALL=(root) NOPASSWD: /usr/bin/pmset -a disablesleep 1, /usr/bin/pmset -a disablesleep 0\n' "$uid" > "$tmp" || exit 73
  chown root:wheel "$tmp" && chmod 0440 "$tmp" || exit 73
  # Nothing is installed unless the new file validates on its own.
  /usr/sbin/visudo -cf "$tmp" >/dev/null 2>&1 || { echo "install-lid-rule.sh: rule rejected by visudo, nothing installed" >&2; exit 65; }
  mv -f "$tmp" "$RULE" || exit 73
  trap - EXIT
  if [ "$legacy" = 1 ]; then
    remove_legacy || echo "install-lid-rule.sh: old helper removal reported a problem" >&2
  fi
  echo "nox-lid-sleep installed"
}

remove_rule() {
  rm -f "$RULE"
  remove_legacy || echo "install-lid-rule.sh: old helper removal reported a problem" >&2
  echo "nox-lid-sleep removed"
}

case "${1:-}" in
  install) install_rule "${2:-}" "${3:-0}" ;;
  remove) remove_rule ;;
  *) echo "usage: install-lid-rule.sh install <uid> <remove-legacy> | remove" >&2; exit 64 ;;
esac
