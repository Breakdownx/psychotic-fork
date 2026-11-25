#!/bin/sh
# Double-click launcher. Strips our own quarantine flag (a downloaded
# tar keeps it) and starts the unlocker from its own folder.
cd "$(dirname "$0")"
xattr -dr com.apple.quarantine ag_unlocker 2>/dev/null || true
exec ./ag_unlocker "$@"
