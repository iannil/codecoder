#!/bin/sh
# Daemon watchdog: check .ccd_stamp.json freshness, alert if stale.
#
# Usage:
#   sh run.sh                              # check CWD
#   sh run.sh /path/to/project             # check specific root
#   WATCHDOG_WEBHOOK=https://hooks.slack... sh run.sh  # with alert
#   WATCHDOG_RESTART_CMD="systemctl restart codecoder" sh run.sh  # with auto-restart

set -e

ROOT="${1:-$CODECODER_ROOT}"
ROOT="${ROOT:-$(pwd)}"
STAMP="$ROOT/.ccd_stamp.json"
MAX_AGE="${WATCHDOG_MAX_AGE:-120}"  # max seconds since last_tick (2x default wg_tick)

if [ ! -f "$STAMP" ]; then
    echo "[watchdog] no stamp file at $STAMP -- daemon may not be running"
    exit 1
fi

LAST_TICK=$(python3 -c "import json; print(json.load(open('$STAMP'))['last_tick'])" 2>/dev/null || \
            grep -o '"last_tick": *[0-9]*' "$STAMP" | head -1 | grep -o '[0-9]*')

if [ -z "$LAST_TICK" ]; then
    echo "[watchdog] cannot read last_tick from $STAMP"
    exit 2
fi

NOW=$(date +%s)
AGE=$((NOW - LAST_TICK))

if [ "$AGE" -lt "$MAX_AGE" ]; then
    echo "[watchdog] OK -- last tick ${AGE}s ago (max ${MAX_AGE}s)"
    exit 0
fi

echo "[watchdog] STALE -- last tick ${AGE}s ago (max ${MAX_AGE}s)"

# Send webhook alert
if [ -n "$WATCHDOG_WEBHOOK" ]; then
    PAYLOAD="{\"text\":\":rotating_light: CodeCoder daemon STALE -- no tick for ${AGE}s (max ${MAX_AGE}s) at $(hostname):${ROOT}\"}"
    curl -s -X POST -H "Content-Type: application/json" -d "$PAYLOAD" "$WATCHDOG_WEBHOOK" || true
fi

# Auto-restart if configured
if [ -n "$WATCHDOG_RESTART_CMD" ]; then
    echo "[watchdog] running restart command: $WATCHDOG_RESTART_CMD"
    eval "$WATCHDOG_RESTART_CMD" || true
fi

exit 1
