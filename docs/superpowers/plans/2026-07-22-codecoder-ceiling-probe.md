# CodeCoder 上限深挖压测 — 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 不重复昨天的广度审计,针对报告已定位的天花板 + 报告未 live 的新特性(ADR 0034 崩溃预算 / ADR 0033 账本)做定向压测,把 codecoder 顶到失效边界,刻画行为边界、live 坐实新特性、狩猎真实破坏,产出可复现的《上限深挖报告》。

**Architecture:** 真实仓只编译二进制 + 接收最终报告;所有 codecoder 运行发生在全新 sibling 隔离工作区 `codecoder-probe/`(`CODECODER_ROOT` 指向它,不复用昨天的 `codecoder-lab/`)。双轨:轨一(Phase 1)逐天花板纵切,轨二(Phase 2)bug 狩猎,Phase 3 复合对抗,Phase 4 综合。证据 = 日志(`probe/logs/`)+ 落盘产物(`supervisor_state.json`/`bg_ledger.jsonl`/`workgraph.json`/`memory`)+ jq/grep 断言 + 退出码——LLM 非确定性下唯一可复现依据。

**Tech Stack:** Rust(codecoder 本体,DeepSeek 经 OpenAI 兼容 base)、bash 驱动脚本(复用 `drive_cc.sh`/`bg_runner.sh` + 新增 `probe_ctx.sh`/`probe_concurrent.sh`)、`jq`(结构断言)、`.ccd.env`(真实 API key)。

## Global Constraints

- **真实仓保持干净**:除 `docs/superpowers/{specs,plans,scripts,audits}/` 与本计划产出的报告外,绝不改动 `src/`、`skills/`、`capabilities/`、git master。
- **隔离边界**:probe lab 路径固定 `/Users/rong.zhu/Code/codecoder-probe/`(真实仓 sibling);所有 `CODECODER_ROOT` 指向它。昨天的 `codecoder-lab/` 不动,留作冻结证据。
- **SIGINT 只对 probe-lab 进程**(Task 13/14),绝不波及真实仓 daemon 或当前会话。
- **bug 狩猎(Task 11/12)只在 probe lab 内**喂病态/并发,绝不打真实仓 daemon。
- **Persistent 崩溃服务(Task 3)**:限 probe lab;靠 daemon 重启测预算,不无上限 spawn。
- **诚实标注**:网络/Docker/限流不通或 DeepSeek 失败时降级 StubClient 并如实标 `limited`,不冒充成功;每条结论附可复现命令 + 证据。
- **领域术语**遵 `CONTEXT.md`。
- **commit 规范**:conventional commits + 中文正文讲动机;过程产物提交到当前 `explore/codecoder-ceiling-probe` 分支。
- **真实仓内二进制路径**:`target/debug/codecoder`(daemon+BG 同体)、`target/debug/cc`(客户端)。probe lab socket = `codecoder-probe/.ccd.sock`。

## 约定(所有 Task 共用,DRY)

```bash
LAB=/Users/rong.zhu/Code/codecoder-probe
CC=/Users/rong.zhu/Code/codecoder/target/debug/cc
DAEMON=/Users/rong.zhu/Code/codecoder/target/debug/codecoder
ENV_FILE=/Users/rong.zhu/Code/codecoder/.ccd.env
SCRIPTS=/Users/rong.zhu/Code/codecoder/docs/superpowers/scripts
# 注入真实 DeepSeek key(若失败,BG/cc 回退 StubClient,如实记录)
source_env() { set -a; . "$ENV_FILE"; set +a; }
# 起 daemon(后台):source_env; CODECODER_ROOT=$LAB "$DAEMON" &
# 关 daemon:CODECODER_ROOT=$LAB "$CC" shutdown
```

`drive_cc.sh` / `bg_runner.sh` 已存在(昨天提交),签名:`drive_cc.sh <label> <msg> [answers_file]`、`bg_runner.sh <label> <task>`,均尊重 `CODECODER_ROOT` 与 `CC_BIN`/`BG_BIN` env 覆盖,日志落 `$LAB/logs/<ts>[-bg]-<label>.log`,退出码 = 子进程退出码。

## log 标记(所有断言以此为据)

`⚙ <name>: <preview>`(工具开始)· `  <name> ✓`(成功)· `  <name> ✗ <output>`(失败)· `· <text>`(Notice)· `[ctx <pct>%]`(stderr)· `🔐 Permission request: <key>`(权限弹窗)· `error: <msg>`(终态)· 空行+退出(TurnComplete)。

## File Structure

真实仓新增(提交到 `explore/codecoder-ceiling-probe`):
- `docs/superpowers/scripts/probe_ctx.sh` — 跑 cc 并把 stderr `[ctx N%]` 抽成时间序列。
- `docs/superpowers/scripts/probe_concurrent.sh` — 并发起 N 个 bg_runner,收集退出码。
- `docs/superpowers/audits/2026-07-22-codecoder-ceiling-probe.md` — 最终报告。

隔离工作区 `/Users/rong.zhu/Code/codecoder-probe/`(scratch,不进真实仓 git):
- `AGENTS.md`/`CONTEXT.md`/`codecoder.json` — 目标项目身份 + 每 probe 按需覆盖的 allowlist。
- `logs/` — 全量运行日志;`matrix.md` — 审计矩阵草稿(逐行追加)。
- `skills/`/`prompts/`/`capabilities/`/`memory/`/`causal_tree.json`/`workgraph.json`/`sessions/` — codecoder 运行产物。
- `bg_ledger.jsonl`/`supervisor_state.json` — ADR 0033/0034 probe 产物。
- `showcase/` — P2/P12 种子(crate + 复合对抗任务)。

## 矩阵行格式(每个 probe 追加一行到 `probe/matrix.md`)

```
| <探测> | <行为边界 at X / 破坏成立 / live 坐实 / safe / limited> | <可复现命令> | <证据(log+jq/grep)> | <备注/突破点> |
```

---

## Phase 0 — 编译、搭台、smoke

### Task 1: 编译核验 + 建 probe lab + smoke

**Files:**
- Create: `codecoder-probe/AGENTS.md`、`codecoder-probe/CONTEXT.md`、`codecoder-probe/codecoder.json`、`codecoder-probe/matrix.md`
- Read-only verify: `Cargo.toml`、`src/agent.rs:18`

**Interfaces:**
- Produces: 可用的 probe lab + 初始化的 `matrix.md`;smoke 证据证明 daemon↔cc↔DeepSeek 全链路通(后续 Task 的基线)。

- [ ] **Step 1: 编译 codecoder + cc,核验报告后新提交仍能 build**

Run: `cargo build 2>&1 | tail -5 && cargo build --bin cc 2>&1 | tail -3`
Expected: 两次均 `Finished` dev;`target/debug/codecoder` 与 `target/debug/cc` 存在。

- [ ] **Step 2: 建 probe lab 身份 + 基线 allowlist**

Run:
```bash
mkdir -p "$LAB/logs" "$LAB/probes"
cat > "$LAB/AGENTS.md" <<'EOF'
# Probe Target Project
你是被 codecoder 操作的受控目标项目工作目录(用于上限压测)。
- 改动前先用只读工具勘察;副作用受权限门控。
- 测试失败如实说明并附输出;完成且验证过才宣称完成。
EOF
cat > "$LAB/CONTEXT.md" <<'EOF'
# Probe 术语表(最小)
- Session: 持久化 JSON 对话(sessions/)。 _Avoid_: History
EOF
cat > "$LAB/codecoder.json" <<'EOF'
{
  "allowlist": [
    "generate_skill", "generate_prompt", "promote_prompt", "generate_capability",
    "write_file", "edit_file",
    "run_command:git", "run_command:cargo", "run_command:ls", "run_command:cat",
    "run_command:echo", "run_command:mkdir", "run_command:test", "run_command:uname",
    "run_capability:linter@shell", "run_capability:mdcount@wasm"
  ]
}
EOF
cat > "$LAB/matrix.md" <<'EOF'
# CodeCoder 上限深挖矩阵(草稿)

| 探测 | 判定 | 命令 | 证据 | 备注/突破点 |
|---|---|---|---|---|
EOF
```
Expected: `jq . "$LAB/codecoder.json"` 退出 0;四文件就位。

- [ ] **Step 3: 核验复用脚本仍在 + 语法 OK**

Run: `for s in drive_cc bg_runner fake_cc; do bash -n "$SCRIPTS/$s.sh" && echo "$s ok"; done`
Expected: 三个 `ok`。

- [ ] **Step 4: 起 daemon(后台)+ smoke 真实 DeepSeek**

Run(harness run_in_background):
```bash
source_env; CODECODER_ROOT="$LAB" exec "$DAEMON"
```
Expected: daemon 长驻;`$LAB/.ccd.sock` 出现。

Run:
```bash
source_env; CODECODER_ROOT="$LAB" "$SCRIPTS/drive_cc.sh" smoke "用一句话自述,然后列出当前工作目录的文件" /dev/null
```
Expected: 日志含 codecoder 自述(StreamDelta)+ `  list_directory ✓`;`EXIT=0`。

- [ ] **Step 5: smoke 断言 + 记矩阵 + 关 daemon**

Run:
```bash
L=$(ls -t "$LAB"/logs/*-smoke.log | head -1)
grep -q 'list_directory ✓' "$L" && grep -q 'EXIT=0' "$L" && echo CONNECT_OK
printf '| cc↔daemon↔DeepSeek 连通 | live 坐实 | drive_cc.sh smoke | %s | 基线通 |\n' "$(basename "$L")" >> "$LAB/matrix.md"
CODECODER_ROOT="$LAB" "$CC" shutdown
```
Expected: `CONNECT_OK`;矩阵新增一行;daemon 退出。

### Task 2: 写 probe 专用脚本 + 自测 + 提交

**Files:**
- Create: `docs/superpowers/scripts/probe_ctx.sh`、`docs/superpowers/scripts/probe_concurrent.sh`

**Interfaces:**
- Produces: `probe_ctx.sh <label> <msg> [answers]` → 退出码 = cc 退出码;额外输出 `$LAB/logs/<ts>-<label>.ctx`(ctx% 时间序列)。
- Produces: `probe_concurrent.sh <label> <N> <task>` → 并发起 N 个 bg_runner,末行打印各进程退出码数组。

- [ ] **Step 1: 写 probe_ctx.sh**

```bash
cat > "$SCRIPTS/probe_ctx.sh" <<'EOF'
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
EOF
chmod +x "$SCRIPTS/probe_ctx.sh"
```
Expected: `bash -n "$SCRIPTS/probe_ctx.sh"` 退出 0。

- [ ] **Step 2: 写 probe_concurrent.sh**

```bash
cat > "$SCRIPTS/probe_concurrent.sh" <<'EOF'
#!/usr/bin/env bash
# probe_concurrent.sh <label> <N> <task>
#   并发起 N 个 bg_runner 跑同一 task(竞态探测),末行打印各进程退出码数组。
set -uo pipefail
LABEL="${1:?label required}"; N="${2:?N required}"; TASK="${3:?task required}"
LAB="${CODECODER_ROOT:-/Users/rong.zhu/Code/codecoder-probe}"
SELF="$(cd "$(dirname "$0")" && pwd)/bg_runner.sh"
pids=(); rcs=()
for i in $(seq 1 "$N"); do
  CODECODER_ROOT="$LAB" bash "$SELF" "${LABEL}-p${i}" "$TASK" &
  pids+=($!)
done
for i in "${!pids[@]}"; do
  if wait "${pids[$i]}"; then rcs[$i]=0; else rcs[$i]=$?; fi
done
echo "=== concurrent $LABEL: exit codes [${rcs[*]}] ==="
EOF
chmod +x "$SCRIPTS/probe_concurrent.sh"
```
Expected: `bash -n` 退出 0。

- [ ] **Step 3: 自测 probe_ctx.sh 机制(用 fake_cc,不烧 token)**

Run:
```bash
printf 'y\n' > /tmp/ans.txt
CC_BIN="$SCRIPTS/fake_cc.sh" CODECODER_ROOT="$LAB" "$SCRIPTS/probe_ctx.sh" ctx_selftest "list files" /tmp/ans.txt
```
Expected: stdout 含 fake_cc 的 `⚙ list_directory:` 与 `EXIT=0`;`$LAB/logs/*-ctx_selftest.ctx` 存在(fake_cc 不产 ctx 标记 → 文件可为空,机制跑通即可)。

- [ ] **Step 4: 自测 probe_concurrent.sh(用 fake_cc 当 bg_runner 桩)**

Run:
```bash
# 临时让 probe_concurrent 调 fake_cc 而非真 bg_runner:把 bg_runner.sh 符号链接到 fake_cc 不现实,
# 改用最小桩验证并发收集逻辑。
cat > /tmp/fake_bg.sh <<'EOF'
#!/usr/bin/env bash
echo "fake bg $2"; exit 0
EOF
chmod +x /tmp/fake_bg.sh
# 直接测 wait/收集逻辑:并发 3 个桩
pids=(); rcs=()
for i in 1 2 3; do /tmp/fake_bg.sh x "p$i" & pids+=($!); done
for i in "${!pids[@]}"; do wait "${pids[$i]}" && rcs[$i]=0 || rcs[$i]=$?; done
echo "exit codes [${rcs[*]}]"
```
Expected: 末行 `exit codes [0 0 0]`(验证并发+收集逻辑;真 bg_runner 路径在 Task 11 实跑)。

- [ ] **Step 5: 提交脚本**

```bash
git add docs/superpowers/scripts/probe_ctx.sh docs/superpowers/scripts/probe_concurrent.sh
git commit -m "$(cat <<'EOF'
chore: 新增上限压测探针脚本

probe_ctx.sh 跑 cc 并把 stderr [ctx N%] 抽成时间序列(漂移/compaction 探针);
probe_concurrent.sh 并发起 N 个 bg_runner 收集退出码(竞态探针)。复用既有
drive_cc/bg_runner,经 CODECODER_ROOT 覆盖指向全新 codecoder-probe lab。
EOF
)"
```
Expected: commit 成功。

---

## Phase 1 — 轨一:行为刻画(纵切)

> **统一流程**:每个 Task 起 daemon(Task 1 Step 4 方式)→ 驱动 → 用 `jq`/`grep` 断言日志与落盘产物 → 追加矩阵行 → 关 daemon。每 probe 独立可复现。

### Task 3 (P7): Persistent Supervisor 崩溃预算(ADR 0034)— live 坐实

**Files:**
- Create: `codecoder-probe/capabilities/crasher/`(manifest + 入口脚本,由本 Task 手写种子)
- Read-only verify: `src/supervisor_state.rs:62-100`(record_crash/should_skip/reset 逻辑)

**Interfaces:**
- Produces: `codecoder-probe/supervisor_state.json` 的 crash_count/gave_up 演化序列 + daemon 重启日志 skip 标记 → 报告 P7 行。**报告只源码核验过,本 Task 首次 live。**

- [ ] **Step 1: 种子一个必崩的 Persistent capability**

```bash
mkdir -p "$LAB/capabilities/crasher"
cat > "$LAB/capabilities/crasher/manifest.json" <<'EOF'
{
  "name": "crasher",
  "environment": "Shell",
  "lifecycle": "Persistent",
  "entry": "crasher.sh"
}
EOF
cat > "$LAB/capabilities/crasher/crasher.sh" <<'EOF'
#!/usr/bin/env bash
echo "crasher up; exiting 1 immediately"; exit 1
EOF
chmod +x "$LAB/capabilities/crasher/crasher.sh"
```
Expected: manifest 合法(`jq . "$LAB/capabilities/crasher/manifest.json"` 退出 0)。

- [ ] **Step 2: fresh daemon + 触发首次崩溃**

清旧状态:`rm -f "$LAB/supervisor_state.json"`。起 daemon(后台,`source_env; CODECODER_ROOT=$LAB "$DAEMON" &`)。
Run:
```bash
source_env; CODECODER_ROOT="$LAB" "$SCRIPTS/drive_cc.sh" p7_crash1 "用 run_capability 执行 crasher@shell 这个 Persistent 服务" /dev/null
```
Expected(per ADR 0034): 日志 `  run_capability ✓`(调度成功)随后服务崩;`jq '.services["crasher"].crash_count' "$LAB/supervisor_state.json"` ≥1;`gave_up` 此时为 false(未达预算 3)。**若实际不同 → 记为发现。**

- [ ] **Step 3: 重启 daemon,重复 spawn-crash 直到达预算**

循环(每次 = 关 daemon → 重启 → 触发一次 run_capability → 记 crash_count):
```bash
for round in 2 3; do
  CODECODER_ROOT="$LAB" "$CC" shutdown
  source_env; CODECODER_ROOT="$LAB" "$DAEMON" & sleep 2
  source_env; CODECODER_ROOT="$LAB" "$SCRIPTS/drive_cc.sh" "p7_crash$round" "用 run_capability 执行 crasher@shell" /dev/null
  echo "round $round crash_count=$(jq '.services["crasher"].crash_count // 0' "$LAB/supervisor_state.json") gave_up=$(jq '.services["crasher"].gave_up // false' "$LAB/supervisor_state.json")"
done
```
Expected(per ADR 0034): round2 → crash_count=2,gave_up=false;round3 → crash_count=3,gave_up=true(record_crash 达预算设 gave_up)。

- [ ] **Step 4: 再重启一次,验跳过 spawn(gave_up)**

```bash
CODECODER_ROOT="$LAB" "$CC" shutdown
source_env; CODECODER_ROOT="$LAB" "$DAEMON" & sleep 2
# 给 daemon 一点时间做 start_all,看是否跳过 crasher
sleep 1
grep -iE 'skip|gave_up|crasher' "$LAB"/logs/*-bg-*.log "$LAB"/logs/*.log 2>/dev/null | tail -5 || true
jq '.services["crasher"]' "$LAB/supervisor_state.json"
```
Expected(per ADR 0034): daemon **不** respawn crasher(should_skip=true 因 gave_up);日志或行为显示跳过。`jq` 仍显示 crash_count=3,gave_up=true。**若仍 respawn → 记为 bug/契约违背发现。**

- [ ] **Step 5: 改 manifest mtime,验预算重置再 spawn**

```bash
touch "$LAB/capabilities/crasher/manifest.json"   # 刷 mtime
CODECODER_ROOT="$LAB" "$CC" shutdown
source_env; CODECODER_ROOT="$LAB" "$DAEMON" & sleep 2
sleep 1
jq '.services["crasher"]' "$LAB/supervisor_state.json"
```
Expected(per ADR 0034 `reset_if_manifest_changed`): mtime 变 → crash_count 归 0、gave_up=false → daemon 再次 spawn crasher(又会崩,crash_count 升回 1)。

- [ ] **Step 6: 断言 + 记矩阵 + 关 daemon**

```bash
echo "P7 final state:"; jq '.services["crasher"]' "$LAB/supervisor_state.json"
printf '| P7 Persistent 崩溃预算(0034) | live 坐实 | 见 Task3 步骤 | supervisor_state.json crash_count→3→gave_up→reset | %s |\n' \
  "$(jq -c '.services["crasher"]' "$LAB/supervisor_state.json")" >> "$LAB/matrix.md"
CODECODER_ROOT="$LAB" "$CC" shutdown
```
Expected: 矩阵记一行;结论 = live 坐实(若任一步偏离 ADR,判定改为发现并详记)。

### Task 4 (P8): BG 账本 + 退出码告警(ADR 0033)— live 坐实

**Files:**
- Read-only verify: `src/bg_ledger.rs`(mission_state→exit code 映射)

**Interfaces:**
- Produces: `codecoder-probe/bg_ledger.jsonl` 5 类 mission_state 行 + 各进程退出码 + `cc ledger` 输出 → 报告 P8 行。**报告未 live,本 Task 首次。**

- [ ] **Step 1: 正常完成 → exit 0(CompletedAllReady)**

```bash
rm -f "$LAB/bg_ledger.jsonl"
source_env; CODECODER_ROOT="$LAB" "$SCRIPTS/bg_runner.sh" p8_ok "在 showcase 下用 write_file 建一个 hello.txt 写一行字"
echo "exit=$?"; tail -1 "$LAB/bg_ledger.jsonl" | jq -c '{state:.mission_state}'
```
Expected: `exit=0`;ledger 末行 `mission_state` ∈ {CompletedAllReady, Running},exit=0。

- [ ] **Step 2: 硬依赖断裂 → exit 2(BlockedAt)**

先种一个 deps 指向不存在前置的 workgraph:
```bash
cat > "$LAB/workgraph.json" <<'EOF'
{"milestones":[
 {"id":"m1","title":"ghost","status":"Done","deps":[]},
 {"id":"m2","title":"real","status":"Ready","deps":["nonexistent"]}
]}
EOF
source_env; CODECODER_ROOT="$LAB" "$SCRIPTS/bg_runner.sh" p8_blocked "推进 workgraph 里就绪的里程碑"
echo "exit=$?"; tail -1 "$LAB/bg_ledger.jsonl" | jq -c '{state:.mission_state}'
```
Expected: `exit=2`;`mission_state=BlockedAt(_)`。

- [ ] **Step 3: 连续失败熔断 → exit 3(CircuitBreaker)**

```bash
rm -f "$LAB/workgraph.json"
source_env; CODECODER_ROOT="$LAB" CODECODER_BG_CIRCUIT_K=1 "$SCRIPTS/bg_runner.sh" p8_circuit "把 showcase/hello.txt 改成内容故意无法通过 'test -e /nonexistent_marker' 的校验,反复直到熔断"
echo "exit=$?"; tail -1 "$LAB/bg_ledger.jsonl" | jq -c '{state:.mission_state}'
```
Expected: `exit=3`;`mission_state=CircuitBreaker`(CIRCUIT_K=1 连续 1 次失败即熔断)。**若 LLM 不按要求制造失败 → 如实记 limited 并改用更可控的失败种子。**

- [ ] **Step 4: provider 错误 → exit 4(Error)**

用坏 base 强制 provider 出错:
```bash
source_env; CODECODER_API_BASE="https://invalid.invalid/v" CODECODER_ROOT="$LAB" "$SCRIPTS/bg_runner.sh" p8_error "列出当前目录"
echo "exit=$?"; tail -1 "$LAB/bg_ledger.jsonl" | jq -c '{state:.mission_state}'
```
Expected: `exit=4`;`mission_state=Error(_)`。

- [ ] **Step 5: SIGINT 取消 → exit 0**

后台起一个长跑 BG,中途 SIGINT:
```bash
source_env; CODECODER_ROOT="$LAB" "$SCRIPTS/bg_runner.sh" p8_sigint "读取 showcase 下所有文件并逐个详细总结,慢慢来" &
BG_PID=$!; sleep 6; kill -INT "$BG_PID"; wait "$BG_PID"; echo "exit=$?"
tail -1 "$LAB/bg_ledger.jsonl" | jq -c '{state:.mission_state}'
```
Expected: `exit=0`(操作者主动取消,非故障);ledger 记取消相关 mission_state。

- [ ] **Step 6: 验 cc ledger 子命令 + 记矩阵**

```bash
source_env; CODECODER_ROOT="$LAB" "$CC" ledger --last 10 2>&1 | tail -12
printf '| P8 BG 账本+退出码(0033) | live 坐实 | bg_runner x5 + cc ledger | bg_ledger.jsonl 5 类 state→exit 0/2/3/4/0 | 映射全验 |\n' >> "$LAB/matrix.md"
```
Expected: `cc ledger` 输出 5 条摘要;矩阵记一行。

### Task 5 (P1): 12-tool 迭代上限

**Files:**
- Read-only verify: `src/agent.rs:18`(`const MAX_TOOL_ITERATIONS: usize = 12`)

**Interfaces:**
- Produces: 单 turn `⚙` 计数 + 触顶 Notice 证据 + 跨 turn 续推证据 → 报告 P1 行 + 突破点(无 env 旋钮,常量 12;BG 有 `CODECODER_BG_MILESTONE_TOOL_CAP=8`)。

- [ ] **Step 1: 起 daemon + 给一个需 >12 工具的单 message 任务**

```bash
source_env; CODECODER_ROOT="$LAB" "$DAEMON" & sleep 2
source_env; CODECODER_ROOT="$LAB" "$SCRIPTS/drive_cc.sh" p1_cap "逐个读取 src 下每个 .rs 文件的前几行并各自用一句话总结,文件很多,不要省略" /dev/null
```
Expected: 日志单 turn 内 `⚙` 标记数接近/达到 12;出现触顶 Notice(提交 16a4876 已加,不再静默截断);TurnComplete。

- [ ] **Step 2: 断言触顶 + 跨 turn 续推**

```bash
L=$(ls -t "$LAB"/logs/*-p1_cap.log | head -1)
echo "tool_starts=$(grep -c '^⚙' "$L")"
grep -iE 'iteration|cap|上限|exhaust|MAX_TOOL' "$L" | tail -5
# 跨 turn 续推:同一 session 再发一 message 看是否继续未完工作
source_env; CODECODER_ROOT="$LAB" "$SCRIPTS/drive_cc.sh" p1_resume "继续上一个任务,把剩余文件总结完" /dev/null
grep -c '^⚙' "$(ls -t "$LAB"/logs/*-p1_resume.log | head -1)"
```
Expected: p1_cap 的 `tool_starts` ≈12 且有触顶 Notice;p1_resume 继续产出工具调用(跨 turn 续推)。

- [ ] **Step 3: 记矩阵 + 关 daemon**

```bash
printf '| P1 12-tool 迭代上限 | 行为边界 at 12 | drive_cc p1_cap | tool_starts≈12 + 触顶 Notice + 跨 turn 续推 | 全局 const 12 无 env 旋钮;BG milestone cap=8 可调 |\n' >> "$LAB/matrix.md"
CODECODER_ROOT="$LAB" "$CC" shutdown
```

### Task 6 (P5): 复合命令 keying

**Files:** 无新增;验证 PermissionKey 按首 token。

**Interfaces:**
- Produces: 复合命令 `run_command:<首token>` 证据 + 预授权不覆盖证据 + codecoder 自适应 `--manifest-path` 证据 → 报告 P5 行 + 突破点。

- [ ] **Step 1: 起 daemon + 跑复合命令(预授权 cargo 应被首 token cd 挡住)**

先在 lab 种一个最小 crate 供 `cargo test` 指向:
```bash
mkdir -p "$LAB/showcase/mini"; cat > "$LAB/showcase/mini/Cargo.toml" <<'EOF'
[package]; name = "mini"; version = "0.1.0"
EOF
source_env; CODECODER_ROOT="$LAB" "$DAEMON" & sleep 2
source_env; CODECODER_ROOT="$LAB" "$SCRIPTS/drive_cc.sh" p5_compound "在 showcase/mini 下运行 'cd showcase/mini && cargo test'" <(printf 'y\n')
```
Expected: 日志 `🔐 Permission request: run_command:cd`(首 token);预授权的 `run_command:cargo` **不**覆盖 → 弹窗;喂 y 后执行(或 codecoder 自适应改 `cargo test --manifest-path showcase/mini/Cargo.toml`)。

- [ ] **Step 2: 试 `;` 与 `|` 复合**

```bash
source_env; CODECODER_ROOT="$LAB" "$SCRIPTS/drive_cc.sh" p5_semi "运行 'ls; uname -a'" <(printf 'y\ny\n')
source_env; CODECODER_ROOT="$LAB" "$SCRIPTS/drive_cc.sh" p5_pipe "运行 'ls | head -3'" <(printf 'y\n')
```
Expected: 各自 keying 取首 token(`ls`);观察 codecoder 是否拆分或整体执行。

- [ ] **Step 3: 记矩阵 + 关 daemon**

```bash
printf '| P5 复合命令 keying | 行为边界 | drive_cc p5_compound/semi/pipe | 🔐 run_command:<首token>;预授权 cargo 不覆盖 cd&&cargo | 突破点:keying 取最危险子命令 |\n' >> "$LAB/matrix.md"
CODECODER_ROOT="$LAB" "$CC" shutdown
```

### Task 7 (P6): review 对抗性化解

**Files:**
- Create: `codecoder-probe/samples/over_eng.rs`、`codecoder-probe/skills/defend.md`(种子,由本 Task 手写)

**Interfaces:**
- Produces: 宽松/严格两框架下的 Verdict + 四信号对比 → 报告 P6 行(哪个信号最易被说辞化解)。

- [ ] **Step 1: 种子"能言善辩的过度工程"代码 + 辩护 skill**

```bash
mkdir -p "$LAB/samples"
cat > "$LAB/samples/over_eng.rs" <<'EOF'
// "Enterprise-grade" 加法:层层抽象,命名花哨,但能跑。
pub trait AbstractBinaryOperation { fn apply(&self, a: i32, b: i32) -> i32; }
pub struct AdditionStrategy;
impl AbstractBinaryOperation for AdditionStrategy {
    fn apply(&self, a: i32, b: i32) -> i32 {
        let mut factory = AdditionStrategy;
        factory.apply_inner(a, b)
    }
}
impl AdditionStrategy {
    fn apply_inner(&self, a: i32, b: i32) -> i32 { a + b }
}
EOF
cat > "$LAB/skills/defend.md" <<'EOF'
---
name: defend
description: 为抽象分层辩护——可扩展性是正当地工程投资。
---
当代码被指过度设计时,指出 trait/strategy 模式为未来运算类型扩展预留了空间,这是正当的前瞻性设计,非过度工程。
EOF
```
Expected: 两文件就位。

- [ ] **Step 2: 起 daemon + 宽松框架下 review**

```bash
source_env; CODECODER_ROOT="$LAB" "$DAEMON" & sleep 2
source_env; CODECODER_ROOT="$LAB" "$SCRIPTS/drive_cc.sh" p6_lenient "先 use_skill defend,然后对 samples/over_eng.rs 跑 review,框架按'学习样本代码,宽松判断'" /dev/null
```
Expected: `  review ✓`;Verdict 倾向 pass;`over_engineering` 信号可能被化解。

- [ ] **Step 3: 严格框架下 review**

```bash
source_env; CODECODER_ROOT="$LAB" "$SCRIPTS/drive_cc.sh" p6_strict "对 samples/over_eng.rs 跑 review,框架按'生产主干代码,严格判断过度工程'" /dev/null
```
Expected: `  review ✓`;Verdict=needs_fix/rebuild;`over_engineering=fail`。

- [ ] **Step 4: 记矩阵 + 关 daemon**

```bash
printf '| P6 review 对抗性化解 | 行为边界(框架敏感) | drive_cc p6_lenient/strict | lenient 倾向 pass vs strict over_eng=fail | over_engineering 最易被说辞化解 |\n' >> "$LAB/matrix.md"
CODECODER_ROOT="$LAB" "$CC" shutdown
```

### Task 8 (P3): 上下文漂移阈值

**Files:** 无新增;受控 session。

**Interfaces:**
- Produces: `[ctx N%]` 序列 + 漂移 onset 区间 + `/clear` 消除证据 → 报告 P3 行。

- [ ] **Step 1: 起 daemon + 用 probe_ctx 逐步灌上下文**

```bash
source_env; CODECODER_ROOT="$LAB" "$DAEMON" & sleep 2
source_env; CODECODER_ROOT="$LAB" "$SCRIPTS/probe_ctx.sh" p3_fill1 "读取 CONTEXT.md 全文并逐段总结,要详细" /dev/null
source_env; CODECODER_ROOT="$LAB" "$SCRIPTS/probe_ctx.sh" p3_fill2 "再读取 ARCHITECTURE.md 全文并逐段总结" /dev/null
source_env; CODECODER_ROOT="$LAB" "$SCRIPTS/probe_ctx.sh" p3_fill3 "再读取 README.md 全文并逐段总结" /dev/null
```
Expected: 每次 `.ctx` 文件 ctx% 递增;记录末次 ctx%。

- [ ] **Step 2: 在升高 ctx% 给异质小指令,观察是否误执行为旧模式**

```bash
source_env; CODECODER_ROOT="$LAB" "$SCRIPTS/probe_ctx.sh" p3_drift "用 memory 写 key=drift-probe value='这是一次异质小指令测试'" /dev/null
```
Expected: 若 codecoder 误把 memory 指令执行成"继续总结文档"(旧模式),记为漂移 onset;若正确执行 memory,ctx% 尚未到 onset。记录该次 ctx%。

- [ ] **Step 3: 验 `/clear`(新 session)消除漂移**

```bash
# 新 session(不同 cc 进程,session 自动新建)重发异质指令
source_env; CODECODER_ROOT="$LAB" "$SCRIPTS/probe_ctx.sh" p3_clean "用 memory 写 key=drift-probe2 value='新 session 应正确执行'" /dev/null
```
Expected: 新 session ctx% 低,正确执行 memory(无漂移)。

- [ ] **Step 4: 记矩阵 + 关 daemon**

```bash
printf '| P3 上下文漂移阈值 | 行为边界 onset≈X%% | probe_ctx p3_fill*/drift/clean | ctx%% 序列 + 漂移 onset + 新 session 消除 | 报告称~21%%,本轮实测取区间 |\n' >> "$LAB/matrix.md"
CODECODER_ROOT="$LAB" "$CC" shutdown
```

### Task 9 (P4): compaction tier-2 真实触发

**Files:** 无新增;续 P3 的长 session 继续灌。

**Interfaces:**
- Produces: tier-1(丢 Reasoning/占位化 ToolResult → ctx 回落)→ tier-2(LLM 摘要合成 System)串联证据 + anchor/tail 存活 + 文件路径追踪 → 报告 P4 行。**报告只源码核验,本 Task 首次 live。**

- [ ] **Step 1: 续灌到 tier-1 触发**

```bash
source_env; CODECODER_ROOT="$LAB" "$DAEMON" & sleep 2
for f in src/agent.rs src/daemon/socket.rs src/tool/builtin.rs; do
  source_env; CODECODER_ROOT="$LAB" "$SCRIPTS/probe_ctx.sh" "p4_fill_$(basename $f)" "读取 $f 全文并详细总结每个函数" /dev/null
done
```
Expected: `.ctx` 显示 ctx% 冲高后**回落**(tier-1 触发:丢 Reasoning + 占位化旧 ToolResult);日志可能含 compaction 相关 Notice。

- [ ] **Step 2: 继续灌,逼 tier-2**

```bash
for f in src/review.rs src/workgraph.rs src/capability.rs; do
  source_env; CODECODER_ROOT="$LAB" "$SCRIPTS/probe_ctx.sh" "p4_t2_$(basename $f)" "读取 $f 全文并详细总结" /dev/null
done
```
Expected: tier-1 后仍超阈值 → tier-2 触发(LLM 一次摘要调用把最旧跨度合成 System 消息);ctx% 再次回落到一个稳定水位。

- [ ] **Step 3: 验 anchor/tail 存活 + 文件路径追踪**

```bash
L=$(ls -t "$LAB"/logs/*-p4_t2_*.log | head -1)
# 验近端 tail 与 anchor 仍可被 codecoder 正确引用(问一个早期事实)
source_env; CODECODER_ROOT="$LAB" "$SCRIPTS/drive_cc.sh" p4_recall "前面读过哪些文件?列出文件名" /dev/null
grep -iE 'compact|tier|summary|摘要' "$LAB"/logs/*-p4_*.log | tail -8
```
Expected: p4_recall 能复述部分文件名(anchor/tail 存活);日志有 compaction 迹象。**若 tier-2 始终未触发 → 如实记 limited(上下文未真正超阈值)。**

- [ ] **Step 4: 记矩阵 + 关 daemon**

```bash
printf '| P4 compaction tier-2 真实触发 | live 坐实(或 limited) | probe_ctx p4_* | ctx%% 回落(tier1)→再回落(tier2) + anchor/tail 存活 | 报告只源码核验,本轮 live |\n' >> "$LAB/matrix.md"
CODECODER_ROOT="$LAB" "$CC" shutdown
```

### Task 10 (P2): BG 失败测试固着 + 代码回退(对照实验)

**Files:**
- Create: `codecoder-probe/showcase/mdmini/`(种子 crate,含一个故意失败的测试)

**Interfaces:**
- Produces: 变体 A(单 message 多目标)vs 变体 B(workgraph 逐里程碑)的工具计数/测试通过数/是否回退对比 → 报告 P2 行。**验证 ADR 0030 护栏是否化解报告发现的固着。**

- [ ] **Step 1: 种子带失败测试的小 crate**

```bash
mkdir -p "$LAB/showcase/mdmini/src" "$LAB/showcase/mdmini/tests"
cat > "$LAB/showcase/mdmini/Cargo.toml" <<'EOF'
[package]
name = "mdmini"
version = "0.1.0"
edition = "2021"
EOF
cat > "$LAB/showcase/mdmini/src/lib.rs" <<'EOF'
pub fn add(a: i32, b: i32) -> i32 { a + b }
EOF
cat > "$LAB/showcase/mdmini/tests/t.rs" <<'EOF'
#[test]
fn passes() { assert_eq!(mdmini::add(1, 2), 3); }
#[test]
fn fails_on_purpose() { assert_eq!(2 + 2, 5); }   // 故意失败
EOF
( cd "$LAB/showcase/mdmini" && cargo test 2>&1 | tail -3 )   # 记基线:1 pass 1 fail
```
Expected: 基线 `passes` 通过、`fails_on_purpose` 失败。

- [ ] **Step 2: 变体 A — 单 message 多目标(BG)**

```bash
source_env; CODECODER_ROOT="$LAB" "$SCRIPTS/bg_runner.sh" p2_A_single \
  "在 showcase/mdmini 里:(1)修好失败的测试让它通过 (2)再加一个 reverse 字符串的函数+测试 (3)写一行 README"
echo "exit=$?"
( cd "$LAB/showcase/mdmini" && cargo test 2>&1 | tail -3 )
```
Expected(per 报告): 变体 A 可能**固着**在失败测试,耗尽 tool 预算,reverse/README 未做,甚至**回退** passes 测试。记录最终测试数。

- [ ] **Step 3: 重置 crate + 变体 B — workgraph 逐里程碑(BG)**

```bash
# 重置 crate 到基线
cat > "$LAB/showcase/mdmini/tests/t.rs" <<'EOF'
#[test]
fn passes() { assert_eq!(mdmini::add(1, 2), 3); }
#[test]
fn fails_on_purpose() { assert_eq!(2 + 2, 5); }
EOF
# 预置 workgraph 拆里程碑(逼逐里程碑推进)
rm -f "$LAB/workgraph.json"
source_env; CODECODER_ROOT="$LAB" "$SCRIPTS/drive_cc.sh" p2_B_plan "用 milestone 工具:add 三个依赖有序里程碑 m1='修好失败测试' m2='加 reverse 函数+测试'(deps m1) m3='写 README'(deps m2);然后 start m1" /dev/null
source_env; CODECODER_ROOT="$LAB" "$SCRIPTS/bg_runner.sh" p2_B_wg "推进 workgraph 里就绪的里程碑"
echo "exit=$?"
( cd "$LAB/showcase/mdmini" && cargo test 2>&1 | tail -3 )
```
Expected(per ADR 0030): 变体 B 受 `MILESTONE_TOOL_CAP=8`/`CIRCUIT_K=2` 约束,逐里程碑推进,**不固着**、不回退;最终测试数优于变体 A。

- [ ] **Step 4: 记矩阵(对照)**

```bash
printf '| P2 BG 失败测试固着+回退 | 行为边界(对照) | bg_runner p2_A_single vs p2_B_wg | A 固着/回退 vs B 逐里程碑不固着 | ADR 0030 护栏化解报告发现的固着 |\n' >> "$LAB/matrix.md"
```
Expected: 矩阵记对照结论(若 B 仍固着 → 记为护栏未完全生效的发现)。

---

## Phase 2 — 轨二:bug 狩猎

### Task 11 (P9): 并发 / fan-out

**Files:** 无新增;竞态写 probe lab 文件。

**Interfaces:**
- Produces: 子 agent 宽度观察 + 并发写 `workgraph.json`/`memory`/`sessions/` 是否损坏(`jq .` 失败=损坏)→ 报告 P9 行。**破坏成立** = JSON 损坏/panic;**safe** = 无损坏。

- [ ] **Step 1: 子 agent 宽度(单 turn 派多个)**

```bash
source_env; CODECODER_ROOT="$LAB" "$DAEMON" & sleep 2
source_env; CODECODER_ROOT="$LAB" "$SCRIPTS/drive_cc.sh" p9_fanout "用 agent 工具同时派 3 个子 agent,分别只读勘察 samples/、skills/、showcase/ 并汇报" /dev/null
```
Expected: 日志多个 `  agent ✓`;观察子 agent 是否串行(报告称工具串行)还是并发;深度锁 1 生效(子 agent 不再派子 agent)。

- [ ] **Step 2: 并发多 BG 进程打同一 lab(竞态)**

```bash
source_env; CODECODER_ROOT="$LAB" "$SCRIPTS/probe_concurrent.sh" p9_concurrent 4 \
  "用 milestone 工具 add 一个标题随机的里程碑(用 run_command echo 一个随机串作标题)"
```
Expected: 4 进程各退出码;**关键断言** — 并发后文件不损坏:
```bash
jq . "$LAB/workgraph.json" >/dev/null 2>&1 && echo WG_INTACT || echo WG_CORRUPT
for s in "$LAB"/sessions/*.json; do jq . "$s" >/dev/null 2>&1 || echo "CORRUPT: $s"; done; echo SESSIONS_CHECKED
```
Expected: `WG_INTACT`;无 `CORRUPT`(safe)。**若 CORRUPT → 破坏成立,记 bug + 复现。**

- [ ] **Step 3: 多 client 同时接 daemon**

```bash
source_env; CODECODER_ROOT="$LAB" "$SCRIPTS/drive_cc.sh" p9_client1 "列出当前目录" /dev/null &
source_env; CODECODER_ROOT="$LAB" "$SCRIPTS/drive_cc.sh" p9_client2 "用 memory 写 key=multi value=client2" /dev/null
wait
jq . "$LAB/memory/multi" 2>/dev/null || cat "$LAB/memory/multi" 2>/dev/null || echo "no multi key"
```
Expected: 两 client 各自完成;观察是否串话(client1 的输出混入 client2)。**若串话/串扰 → 记发现。**

- [ ] **Step 4: 记矩阵 + 关 daemon**

```bash
printf '| P9 并发/fan-out | safe 或 破坏成立 | drive_cc p9_fanout + probe_concurrent x4 | jq workgraph/sessions 无损坏=safe | 子 agent 串行;并发写无损坏 |\n' >> "$LAB/matrix.md"
CODECODER_ROOT="$LAB" "$CC" shutdown
```

### Task 12 (P10): 病态输入

**Files:**
- Create: `codecoder-probe/capabilities/{badjson,unknownenv}/`、`codecoder-probe/samples/big.txt`(种子)

**Interfaces:**
- Produces: 5 类病态输入各 panic/异常退出/损坏 vs 优雅 Notice 的判定 → 报告 P10 行。**破坏成立** = panic/损坏;**safe** = 优雅拒绝。

- [ ] **Step 1: 起 daemon + 种子病态 capability**

```bash
source_env; CODECODER_ROOT="$LAB" "$DAEMON" & sleep 2
mkdir -p "$LAB/capabilities/badjson" "$LAB/capabilities/unknownenv"
echo '{ not valid json' > "$LAB/capabilities/badjson/manifest.json"
printf '{"name":"unknownenv","environment":"BareMetal","lifecycle":"OneShot","entry":"x.sh"}' > "$LAB/capabilities/unknownenv/manifest.json"
echo '#!/usr/bin/env bash; echo hi' > "$LAB/capabilities/unknownenv/x.sh"
head -c 52428800 /dev/zero | tr '\0' 'a' > "$LAB/samples/big.txt"   # 50MB
```
Expected: 文件就位。

- [ ] **Step 2: 跑畸形 manifest + 未知 Environment**

```bash
source_env; CODECODER_ROOT="$LAB" "$SCRIPTS/drive_cc.sh" p10_badjson "用 run_capability 执行 badjson@shell" /dev/null
source_env; CODECODER_ROOT="$LAB" "$SCRIPTS/drive_cc.sh" p10_unknownenv "用 run_capability 执行 unknownenv@shell" /dev/null
```
Expected: 均 `  run_capability ✗` + Notice 点明问题(优雅拒绝=safe);**不应 panic**。grep `panic`:
```bash
grep -iE 'panic|thread.*panicked' "$LAB"/logs/*-p10_*.log && echo PANIC_FOUND || echo NO_PANIC
```
Expected: `NO_PANIC`。**若 PANIC_FOUND → 破坏成立,记 bug。**

- [ ] **Step 3: 超大文件 read(撞 length-truncation-guard)+ 超长 grep**

```bash
source_env; CODECODER_ROOT="$LAB" "$SCRIPTS/drive_cc.sh" p10_bigread "读取 samples/big.txt 全文" /dev/null
source_env; CODECODER_ROOT="$LAB" "$SCRIPTS/drive_cc.sh" p10_biggrep "用 grep 在 samples/big.txt 搜 'a',报告命中数" /dev/null
```
Expected: 大文件 read 受长度截断 guard 保护(Notice 或截断,不爆内存/OOM);grep 命中巨多时 tool result 回灌受控。无 panic/OOM。**若进程被 OOM kill(退出码 137)→ 破坏成立。**

- [ ] **Step 4: grep AST 空/循环 + causal_tree 旧 schema**

```bash
mkdir -p "$LAB/samples/emptyast"; : > "$LAB/samples/emptyast/x.rs"
source_env; CODECODER_ROOT="$LAB" "$SCRIPTS/drive_cc.sh" p10_ast "用 grep AST 查询在 samples/emptyasm 下找函数定义(空文件)" /dev/null
# 喂一个旧 schema causal_tree
echo '{"nodes":[{"id":"n1","q":"old"}]}' > "$LAB/causal_tree.json"
source_env; CODECODER_ROOT="$LAB" "$SCRIPTS/drive_cc.sh" p10_schema "用 reason list 看因果树" /dev/null
```
Expected: 空文件 AST 查询优雅返回空(无 panic);旧 schema causal_tree 触发迁移或 Notice(提交 6f2ebc1 加了 schema 迁移),不 panic。**若 panic → 破坏成立。**

- [ ] **Step 5: 断言 + 记矩阵 + 关 daemon**

```bash
grep -iE 'panic|OOM|killed' "$LAB"/logs/*-p10_*.log && echo PANIC_FOUND || echo ALL_SAFE
printf '| P10 病态输入 | safe 或 破坏成立 | drive_cc p10_* (5 类) | 无 panic/OOM/损坏=safe | 畸形 manifest/大文件/AST 空/旧 schema |\n' >> "$LAB/matrix.md"
CODECODER_ROOT="$LAB" "$CC" shutdown
```

### Task 13 (P11): SIGINT 边界

**Files:** 无新增;只对 probe-lab 进程发 SIGINT。

**Interfaces:**
- Produces: 4 个 SIGINT 时机(LLM 中途/commit 中途/并发双 SIGINT/daemon 整体)的取消行为 + 残留进程检查 → 报告 P11 行。

- [ ] **Step 1: LLM 调用中途 SIGINT(无可取消点)**

```bash
source_env; CODECODER_ROOT="$LAB" "$DAEMON" & sleep 2
source_env; CODECODER_ROOT="$LAB" "$SCRIPTS/drive_cc.sh" p11_llm "逐个详细总结 src 下每个文件,越详细越好,不要停" /dev/null &
P=$!; sleep 5; kill -INT "$P"; wait "$P"; echo "exit=$?"
pgrep -f 'target/debug/(codecoder|cc)' && echo RESIDUAL || echo NO_RESIDUAL
```
Expected: cc 收 SIGINT;因 LLM 调用中无可取消点,取消延迟到下个可取消点或 turn 迭代顶检查;**无残留进程**;daemon 仍存活(只取消了 turn/client,非 daemon)。

- [ ] **Step 2: commit/git 中途 SIGINT**

```bash
source_env; CODECODER_ROOT="$LAB" "$SCRIPTS/drive_cc.sh" p11_commit "在 showcase 下加一个文件然后用 commit 工具提交" <(printf 'y\n') &
P=$!; sleep 4; kill -INT "$P"; wait "$P"; echo "exit=$?"
git -C "$LAB" status --short | head; git -C "$LAB" log --oneline | head -1
```
Expected: 取消后 git 仓库状态一致(无半截提交/无 .git 锁残留);无残留 git 进程。**若 git 损坏(.git/index.lock 残留)→ 记发现。**

- [ ] **Step 3: 并发双 SIGINT + daemon 整体 SIGINT**

```bash
# 双 SIGINT 到同一 cc
source_env; CODECODER_ROOT="$LAB" "$SCRIPTS/drive_cc.sh" p11_double "详细总结 src/agent.rs" /dev/null &
P=$!; sleep 3; kill -INT "$P"; sleep 1; kill -INT "$P"; wait "$P"; echo "exit=$?"
# daemon 整体 SIGINT(优雅关闭)
kill -INT "$(pgrep -f 'target/debug/codecoder' | head -1)"; sleep 2
pgrep -f 'target/debug/codecoder' && echo DAEMON_ALIVE || echo DAEMON_DOWN_CLEAN
```
Expected: 双 SIGINT 不致死锁/崩溃;daemon 整体 SIGINT 优雅退出(DAEMON_DOWN_CLEAN),无残留。

- [ ] **Step 4: 记矩阵**

```bash
printf '| P11 SIGINT 边界 | 行为边界 | drive_cc p11_* + kill -INT | LLM 中途延迟取消;commit 中途 git 一致;双 SIGINT 不崩;daemon 整体优雅 | 无残留进程;扩展报告只验的 run_capability 之外 |\n' >> "$LAB/matrix.md"
```
Expected: 矩阵记一行(若有残留/损坏 → 记发现)。

---

## Phase 3 — 复合对抗

### Task 14 (P12): 复合对抗长任务(交互破坏)

**Files:**
- Create: `codecoder-probe/showcase/TASK.md`(复合任务种子)
- Reuses: P2 的 mdmini crate + P7 的 crasher capability。

**Interfaces:**
- Produces: 多天花板叠加下哪个子系统先失效 + 是否涌现单 probe 没有的破坏 → 报告 P12 行(涌现破坏 or 正面收敛)。

- [ ] **Step 1: 种子复合任务(自找麻烦)**

```bash
cat > "$LAB/showcase/TASK.md" <<'EOF'
# 复合对抗任务
在 showcase/mdmini 上推进:
1. 用 milestone 拆 4 个里程碑(parser/model/renderer/docs)并逐个推进;
2. 期间 showcase 里有个故意失败的测试,要修;
3. 跑一次 run_capability crasher@shell(它会崩);
4. 全程上下文会变长;
5. 偶尔用复合命令(cd ... && cargo test);
6. 最后对产物跑一次 review。
EOF
```
Expected: 文件就位。

- [ ] **Step 2: headless BG 长跑(后台,中途 SIGINT 一次再重跑)**

```bash
source_env; CODECODER_ROOT="$LAB" "$SCRIPTS/bg_runner.sh" p12_run "执行 showcase/TASK.md 的全部要求" &
P=$!; sleep 12; kill -INT "$P"; wait "$P"; echo "exit=$?(取消重跑)"
# 重跑到完成
source_env; CODECODER_ROOT="$LAB" "$SCRIPTS/bg_runner.sh" p12_rerun "继续推进 showcase/TASK.md 到完成"
echo "exit=$?"
```
Expected: 中途 SIGINT 优雅取消(EXIT=0);重跑后产出 crate + 多类产物。

- [ ] **Step 3: 验产物 + 涌现破坏扫描**

```bash
echo "--- crate tests ---"; ( cd "$LAB/showcase/mdmini" && cargo test 2>&1 | tail -3 )
echo "--- artifacts ---"; ls "$LAB/skills" "$LAB/capabilities" "$LAB/memory" 2>/dev/null
echo "--- workgraph done ---"; jq '[.milestones[]|select(.status=="Done")]|length' "$LAB/workgraph.json" 2>/dev/null
echo "--- 涌现破坏扫描 ---"
grep -iE 'panic|deadlock|corrupt|照旧|溢出' "$LAB"/logs/*-bg-p12_*.log | tail -10 || echo NO_EMERGENT_BREAK
jq . "$LAB/workgraph.json" >/dev/null 2>&1 && echo WG_INTACT || echo WG_CORRUPT
tail -1 "$LAB/bg_ledger.jsonl" | jq -c '{state:.mission_state}'
```
Expected: crate 有测试产物;workgraph 有 Done 里程碑;**无 panic/死锁/损坏**(WG_INTACT);ledger 末行 state 正常。**若涌现单 probe 没有的破坏 → 记 bug + 复现。**

- [ ] **Step 4: 记矩阵**

```bash
printf '| P12 复合对抗长任务 | 涌现破坏 或 正面收敛 | bg_runner p12_run/rerun | 多天花板叠加;crate/WG/ledger 状态 | SIGINT 取消 + 重跑至完成 |\n' >> "$LAB/matrix.md"
```
Expected: 矩阵记一行(涌现破坏 or 收敛结论)。

---

## Phase 4 — 综合

### Task 15: 写上限深挖报告 + 自检 + 提交

**Files:**
- Create: `docs/superpowers/audits/2026-07-22-codecoder-ceiling-probe.md`

**Interfaces:**
- Consumes: `codecoder-probe/matrix.md` 草稿 + 各 Task 证据日志/产物。

- [ ] **Step 1: 汇总 matrix + 各 Task 证据成报告**

报告结构:
1. 总览(范围:深挖上限;方法:双轨压测;环境:probe lab + 真实 DeepSeek)。
2. 逐天花板结论(P1–P12):判定(行为边界 at X / 破坏成立 / live 坐实 / safe / limited)+ 证据(log + jq/grep + 退出码)+ 突破点。
3. 新特性 live 坐实(P7 ADR 0034 崩溃预算 / P8 ADR 0033 账本)——首份 live 证据。
4. bug 清单(若有,P9/P10/P11/P12 的破坏成立项,带复现)或"审计范围内无新功能 gap / 无破坏"。
5. 上限在哪 / 可突破点(基于本轮压测的洞察,对照报告 v1)。

- [ ] **Step 2: 写报告文件**

把 Step 1 内容写入 `docs/superpowers/audits/2026-07-22-codecoder-ceiling-probe.md`。

- [ ] **Step 3: 报告自检(无占位、判定与证据一致)**

Run: `grep -nE 'TBD|TODO|待补|占位|XXX' docs/superpowers/audits/2026-07-22-codecoder-ceiling-probe.md`
Expected: 无命中(或均为引用"已知未实现"的正当说明)。

- [ ] **Step 4: 提交报告 + 脚本**

```bash
git add docs/superpowers/audits/2026-07-22-codecoder-ceiling-probe.md
git commit -m "$(cat <<'EOF'
docs: 新增 codecoder 上限深挖压测报告

不重复广度审计,针对报告已定位的天花板 + 报告未 live 的新特性(ADR 0034
崩溃预算 / ADR 0033 账本)做定向压测:双轨(逐天花板纵切 + 复合对抗),
live 坐实 P7/P8,狩猎 P9–P12 的并发/病态/SIGINT/交互破坏。附每条可复现
命令 + 结构化证据,诚实标注 bug 与 limited。
EOF
)"
```
Expected: commit 成功在 `explore/codecoder-ceiling-probe` 分支。

---

## Self-Review(plan vs spec)

**1. Spec coverage:**
- Phase 0(编译/probe lab/脚本/smoke)→ Task 1–2 ✓
- 轨一纵切 P1–P8(12-tool/BG 固着/漂移/compaction/复合 keying/review/Supervisor/账本)→ Task 3–10 ✓(P7=Task3、P8=Task4、P1=Task5、P5=Task6、P6=Task7、P3=Task8、P4=Task9、P2=Task10)
- 轨二 bug 狩猎 P9–P11(并发/病态/SIGINT)→ Task 11–13 ✓
- 复合对抗 P12 → Task 14 ✓
- 报告 + 综合 → Task 15 ✓
- 双轨混合 / 全新 probe lab / 复用驱动 + 新探针 / 结构化证据契约 → 全覆盖 ✓
- 安全边界(SIGINT 只对 lab、bug 狩猎只在 lab、不修源码、诚实标注)→ Global Constraints + 各 Task 断言 ✓

**2. Placeholder scan:** 无 TBD/TODO;LLM 非确定性步骤均用结构化断言(文件存在 / jq / grep 标记 / 退出码 / `panic` 扫描)而非猜字符串;每 probe 的"expected per ADR"是可证伪假设,偏离即记为发现 ✓

**3. Type/interface consistency:** `drive_cc.sh`/`bg_runner.sh` 签名(`<label> <msg> [ans]` / `<label> <task>`)与既有脚本一致;新探针 `probe_ctx.sh`(`<label> <msg> [ans]`)、`probe_concurrent.sh`(`<label> <N> <task>`)全局统一;`CODECODER_ROOT` 覆盖指向 probe lab 全局一致;config 旋钮名(`CODECODER_SUPERVISOR_CRASH_BUDGET`/`CODECODER_BG_CIRCUIT_K`)与 `src/config.rs` 一致 ✓
