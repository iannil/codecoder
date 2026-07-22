#!/usr/bin/env bash
# probe_ctx.sh <label> <message> [answers_file]
#   跑 one-shot cc,stdout+stderr tee 到 log,并把 stderr 的 [ctx N%] 抽成时间序列到 <log>.ctx。
set -uo pipefail
LABEL="${1:?label required}"; MSG="${2:?message required}"; ANSWERS="${3:-/dev/null}"
LAB="${CODECODER_ROOT:-/Users/rong.zhu/Code/codecoder-probe}"
CC_BIN="${CC_BIN:-/Users/rong.zhu/Code/codecoder/target/debug/cc}"
TS="$(date +%Y%m%d-%H%M%S)"
LOG="$LAB/logs/${TS}-${LABEL}.log"; CTX="$LAB/logs/${TS}-${LABEL}.ctx"
mkdir -p "$LAB/logs"
{ echo "=== probe_ctx $TS | $LABEL ==="; echo "MSG: $MSG"; } > "$LOG"
CODECODER_ROOT="$LAB" "$CC_BIN" "$MSG" < "$ANSWERS" > "$LOG.body" 2>&1
RC=$?
cat "$LOG.body" | tee -a "$LOG" >/dev/null
grep -oE '\[ctx [0-9]+%\]' "$LOG" > "$CTX" || true
echo "EXIT=$RC" | tee -a "$LOG"
rm -f "$LOG.body"
echo "=== ctx series ($CTX) ==="; cat "$CTX"
exit $RC
