#!/bin/sh
# Removes the Nox closed-lid helper. Must run as root:
#   sudo scripts/uninstall-helper.sh
# Nox runs the same script (embedded) from "Désinstaller l'assistant".
#
# Lid-close sleep is restored first if Nox had disabled it; a value set by
# the user is left alone. See docs/SLEEP_RECOVERY.md.
set -u
PATH=/usr/bin:/bin:/usr/sbin:/sbin
export PATH

LABEL=io.github.martingaltier.nox.helper
DST=/Library/PrivilegedHelperTools/$LABEL
PLIST=/Library/LaunchDaemons/$LABEL.plist
SOCK=/var/run/$LABEL.sock
STATE_DIR="/Library/Application Support/Nox"
JOURNAL="$STATE_DIR/helper-state.json"

[ "$(id -u)" = 0 ] || { echo "uninstall-helper.sh: must run as root" >&2; exit 77; }

# SIGTERM: a running helper restores lid sleep before exiting.
launchctl bootout "system/$LABEL" 2>/dev/null || true

# Not running (or failed to restore): replay the journal one last time.
status=0
if [ -f "$JOURNAL" ] && [ -x "$DST" ]; then
  "$DST" --recover || status=$?
fi

rm -f "$PLIST" "$DST" "$SOCK"
if [ -f "$JOURNAL" ]; then
  echo "uninstall-helper.sh: lid sleep may still be disabled. Check with" >&2
  echo "  pmset -g | grep SleepDisabled" >&2
  echo "and restore with: sudo pmset -a disablesleep 0" >&2
  rm -f "$JOURNAL"
  status=1
fi
rmdir "$STATE_DIR" 2>/dev/null || true
echo "nox-helper uninstalled"
exit "$status"
