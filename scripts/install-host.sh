#!/bin/sh
# Native host installer. Safety checks and state transitions live in install_host.py.
set -eu
REPO=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
exec python3 "$REPO/scripts/install_host.py" "$@"
