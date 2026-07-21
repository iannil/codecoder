#!/usr/bin/env bash
# drive_cc.sh <label> <message> [answers_file]
#   以 CODECODER_ROOT=lab 跑 one-shot `cc "<message>"`,把 answers_file
#   (默认 /dev/null)直接重定向到 cc 的 stdin。cc 的 prompt_user 只在
#   权限/ask/confirm/plan/trust 弹窗时 read_line,故按序喂 y/n/s/p/N 或自由文本。
#   stdout+stderr tee 到 lab/logs/<ts>-<label>.log;退出码 = cc 退出码。
set -uo pipefail
LABEL="${1:?label required}"; MSG="${2:?message required}"; ANSWERS="${3:-/dev/null}"
LAB="${CODECODER_ROOT:-/Users/rong.zhu/Code/codecoder-lab}"
CC_BIN="${CC_BIN:-/Users/rong.zhu/Code/codecoder/target/debug/cc}"
TS="$(date +%Y%m%d-%H%M%S)"
LOG="$LAB/logs/${TS}-${LABEL}.log"
mkdir -p "$LAB/logs"
{ echo "=== drive_cc $TS | $LABEL ==="; echo "MSG: $MSG"; } > "$LOG"
CODECODER_ROOT="$LAB" "$CC_BIN" "$MSG" < "$ANSWERS" > "$LOG.body" 2>&1
RC=$?
cat "$LOG.body" | tee -a "$LOG"
echo "EXIT=$RC" | tee -a "$LOG"
rm -f "$LOG.body"
exit $RC
