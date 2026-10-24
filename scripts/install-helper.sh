#!/bin/sh
# Installs the Nox closed-lid helper (root LaunchDaemon). Must run as root.
#
# Nox runs this script once, embedded in the app, through the macOS
# administrator prompt (osascript "with administrator privileges") after the
# user clicks "Installer l'assistant". It can also be run by hand:
#   sudo scripts/install-helper.sh /Applications/Nox.app
#
# It never touches sudoers and stores no password. See docs/SLEEP_RECOVERY.md.
set -eu
PATH=/usr/bin:/bin:/usr/sbin:/sbin
export PATH

LABEL=io.github.martingaltier.nox.helper
DST=/Library/PrivilegedHelperTools/$LABEL
PLIST=/Library/LaunchDaemons/$LABEL.plist
SOCK=/var/run/$LABEL.sock
STATE_DIR="/Library/Application Support/Nox"
LOG_DIR=/Library/Logs/Nox

APP=${1:-}
[ -n "$APP" ] || { echo "usage: install-helper.sh /path/to/Nox.app" >&2; exit 64; }
[ "$(id -u)" = 0 ] || { echo "install-helper.sh: must run as root" >&2; exit 77; }

SRC="$APP/Contents/Library/LaunchServices/nox-helper"
CLIENT="$APP/Contents/MacOS/nox"
if [ ! -f "$SRC" ] || [ -L "$SRC" ]; then
  echo "install-helper.sh: helper not found in $APP" >&2; exit 66
fi
[ -x "$CLIENT" ] || { echo "install-helper.sh: $CLIENT not found" >&2; exit 66; }
codesign --verify --strict "$SRC" || { echo "install-helper.sh: helper signature invalid" >&2; exit 65; }

# Stop a previous helper first. On SIGTERM it restores lid sleep if it had
# changed it (and its journal is replayed by the new one otherwise).
launchctl bootout "system/$LABEL" 2>/dev/null || true

install -d -o root -g wheel -m 0755 /Library/PrivilegedHelperTools
install -d -o root -g wheel -m 0755 "$STATE_DIR" "$LOG_DIR"
TMP="$DST.new.$$"
install -o root -g wheel -m 0755 "$SRC" "$TMP"
mv -f "$TMP" "$DST"

# The helper writes its own plist (paths XML-escaped by tested code).
TMP="$PLIST.new.$$"
"$DST" --print-launchd-plist --program "$DST" --allowed-client "$CLIENT" > "$TMP"
plutil -lint -s "$TMP"
chown root:wheel "$TMP"
chmod 0644 "$TMP"
mv -f "$TMP" "$PLIST"

launchctl enable "system/$LABEL"
launchctl bootstrap system "$PLIST"

i=0
while [ ! -S "$SOCK" ] && [ "$i" -lt 50 ]; do
  sleep 0.1
  i=$((i + 1))
done
[ -S "$SOCK" ] || { echo "install-helper.sh: helper did not start (see $LOG_DIR/nox-helper.log)" >&2; exit 70; }
echo "nox-helper installed"
