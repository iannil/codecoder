# CodeCoder 能力探索与上限压测 — 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 编译并启动 codecoder,系统性审计全部内置能力(广度),再用一个最大化复杂的端到端任务把最差异化的特性链起来压测(深度),产出可复现的《能力矩阵 + 上限报告》。

**Architecture:** 真实仓只编译二进制 + 接收最终报告;所有 codecoder 运行发生在 sibling 隔离工作区 `codecoder-lab/`(`CODECODER_ROOT` 指向它)。交互式能力经 `cc "<msg>"` one-shot 模式驱动(五类 Dialog 用管道喂 `y/n/s/p/N`/自由文本 应答);headless 能力经 `CODECODER_BG_TASK` 驱动。证据 = 日志(`lab/logs/`)+ 落盘产物(`lab/skills|capabilities|memory|causal_tree.json|workgraph.json`)+ jq 断言。

**Tech Stack:** Rust(codecoder 本体,DeepSeek 经 OpenAI 兼容 base)、bash 驱动脚本、`jq`(结构断言)、`.ccd.env`(真实 API key)。

## Global Constraints

- **真实仓保持干净**:除 `docs/superpowers/{specs,plans,scripts,audits}/` 与本计划产出的报告外,绝不改动 `src/`、`skills/`、`capabilities/`、git master。
- **隔离边界**:lab 路径固定 `/Users/rong.zhu/Code/codecoder-lab/`(真实仓 sibling);所有 `CODECODER_ROOT` 指向它。
- **SIGINT 只对 lab 的 background task 发**,绝不波及真实仓或当前会话。
- **诚实标注**:网络/Docker/限流不通时标 `limited`/`unimplemented`,不冒充成功;每条结论附可复现命令 + 证据。
- **领域术语**遵 `CONTEXT.md`(Mode/Dialog/Popup、Session vs History、MessageId vs ToolCall.id、Slash vs Agent Command、Permission Scope)。
- **commit 规范**遵 `skills/commit-conventions.md`(conventional commits + 中文正文讲动机);过程产物提交到 `explore/codecoder-capability-audit` 分支。
- **真实仓内二进制路径**:`target/debug/codecoder`(daemon+BG 同体)、`target/debug/cc`(客户端)。lab 的 socket = `codecoder-lab/.ccd.sock`。

## 确定的 log 标记(所有断言以此为据)

`cc` 经 `print_event` 输出:`⚙ <name>: <preview>`(工具开始)· `  <name> ✓`(工具成功,stdout)· `  <name> ✗ <output>`(工具失败,stderr)· `· <text>`(Notice)· `· [<src>] <text>`(BusNotice)· `[ctx <pct>%]`(stderr)· `🔐 Permission request: <key>`(权限弹窗)· `error: <msg>`(Error,终态)· 空行+退出(TurnComplete)。

## File Structure

真实仓新增(均提交到 `explore/codecoder-capability-audit` 分支):
- `docs/superpowers/scripts/drive_cc.sh` — one-shot cc 驱动器(piped-stdin 应答 + tee 日志)。
- `docs/superpowers/scripts/bg_runner.sh` — headless BG 驱动器。
- `docs/superpowers/scripts/fake_cc.sh` — drive_cc.sh 自测桩(不烧 LLM token)。
- `docs/superpowers/audits/2026-07-21-codecoder-capability-matrix.md` — 最终报告。

隔离工作区 `/Users/rong.zhu/Code/codecoder-lab/`(scratch,不进真实仓 git):
- `AGENTS.md` / `CONTEXT.md` — 目标项目身份(codecoder 的「自我」)。
- `codecoder.json` — 预授权 allowlist。
- `logs/` — 全量运行日志。
- `matrix.md` — 审计矩阵草稿(逐行追加)。
- `skills/` `prompts/` `capabilities/` `memory/` `causal_tree.json` `workgraph.json` `sessions/` — codecoder 运行产物。
- `showcase/mdslides/` — 深度展示目标 Rust crate。

## 矩阵行格式(Phase 1/2 每个验证追加一行到 `lab/matrix.md`)

```
| <能力> | <works/limited/unimplemented> | <可复现命令> | <证据(log 文件名 + jq/grep 命中)> | <备注> |
```

---

## Phase 0 — 编译、验证、搭台子

### Task 1: 编译二进制 + 跑测试 + 核验计数声明

**Files:**
- Create: `codecoder-lab/matrix.md`(初始化矩阵表头)
- Read-only verify: `Cargo.toml`、`src/lib.rs`

- [ ] **Step 1: 编译 codecoder + cc**

Run: `cargo build 2>&1 | tail -5`
Expected: `Finished` debug target;无 error。产物 `target/debug/codecoder`、`target/debug/cc` 存在。

- [ ] **Step 2: 跑测试套件,记录实际计数**

Run: `cargo test 2>&1 | tail -20 > /tmp/cc_test_count.txt; cat /tmp/cc_test_count.txt`
Expected: 末尾 `test result: ok. <N> passed; 0 failed; <M> ignored`。记录 `N` 与 `M`。

- [ ] **Step 3: 判定文档计数真值**

核对: `CLAUDE.md` 称 244+3,`ARCHITECTURE.md`/`README.md` 称 202+3。用 Step 2 的 `N`/`M` 判定哪个文档过时。结论写入矩阵「文档计数」行。
Run: `grep -n "202\|244\|247" CLAUDE.md ARCHITECTURE.md README.md`
Expected: 命中各自声称的数字。

- [ ] **Step 4: 初始化 lab + 矩阵**

Run:
```bash
mkdir -p /Users/rong.zhu/Code/codecoder-lab/logs
cat > /Users/rong.zhu/Code/codecoder-lab/matrix.md <<'EOF'
# CodeCoder 能力矩阵(草稿)

| 能力 | 状态 | 命令 | 证据 | 备注 |
|---|---|---|---|---|
EOF
```
Expected: `codecoder-lab/matrix.md` 含表头。

- [ ] **Step 5: 提交构建核验记录**

把计数结论先记一行到矩阵;本任务不产生真实仓代码改动,跳过 commit(真实仓无变更)。

### Task 2: 搭 lab 身份 + 预授权

**Files:**
- Create: `codecoder-lab/AGENTS.md`、`codecoder-lab/CONTEXT.md`、`codecoder-lab/codecoder.json`

- [ ] **Step 1: 写 lab 的 AGENTS.md(目标项目身份)**

内容:声明 lab 是一个「用于被 codecoder 操作的示例目标项目」,要求 codecoder 改动前先只读勘察、危险操作先确认、忠实汇报(沿用真实仓 AGENTS.md 风格但指向 lab)。
Run:
```bash
cat > /Users/rong.zhu/Code/codecoder-lab/AGENTS.md <<'EOF'
# Lab Target Project

你是被 codecoder 操作的示例目标项目的工作目录。本文件即项目身份声明。
- 改动前先用只读工具(read/list/glob/grep)勘察。
- 写文件、运行命令等副作用受权限门控;不确定用 ask_user/confirm。
- 测试失败如实说明并附输出;完成且验证过才宣称完成。
EOF
```
Expected: 文件写入成功。

- [ ] **Step 2: 写最小 CONTEXT.md**

Run:
```bash
cat > /Users/rong.zhu/Code/codecoder-lab/CONTEXT.md <<'EOF'
# Lab 术语表(最小)

- Session: 持久化 JSON 对话(sessions/)。 _Avoid_: History
- History: 内存输入缓冲区。 _Avoid_: Session
EOF
```
Expected: 文件写入成功。

- [ ] **Step 3: 写 codecoder.json 预授权(Phase 1 宽松版)**

Run:
```bash
cat > /Users/rong.zhu/Code/codecoder-lab/codecoder.json <<'EOF'
{
  "allowlist": [
    "generate_skill", "generate_prompt", "promote_prompt", "generate_capability",
    "write_file", "edit_file",
    "run_command:git", "run_command:cargo", "run_command:ls", "run_command:cat", "run_command:echo", "run_command:mkdir", "run_command:test",
    "run_capability:mdcount@wasm", "run_capability:linter@shell"
  ]
}
EOF
```
Expected: 合法 JSON(`jq . codecoder-lab/codecoder.json` 退出 0)。

- [ ] **Step 4: 不提交(lab 不进真实仓 git)**

lab 是 sibling scratch 目录,不归真实仓管,跳过 commit。

### Task 3: 写驱动脚本 + 自测

**Files:**
- Create: `docs/superpowers/scripts/drive_cc.sh`、`docs/superpowers/scripts/bg_runner.sh`、`docs/superpowers/scripts/fake_cc.sh`

**Interfaces:**
- Produces: `drive_cc.sh <label> <message> [answers_file]` → 退出码 = cc 退出码;日志 `codecoder-lab/logs/<ts>-<label>.log`。
- Produces: `bg_runner.sh <label> <task>` → 退出码 = codecoder 退出码;日志 `codecoder-lab/logs/<ts>-bg-<label>.log`。

- [ ] **Step 1: 写 fake_cc.sh 自测桩**

```bash
cat > docs/superpowers/scripts/fake_cc.sh <<'EOF'
#!/usr/bin/env bash
# 模拟 cc one-shot:读 stdin 一行作 prompt 应答,打印工具事件标记后 TurnComplete。
read -r ans
echo "⚙ list_directory: ." 
echo "  list_directory ✓"
echo "(turn complete)"
exit 0
EOF
chmod +x docs/superpowers/scripts/fake_cc.sh
```
Expected: 文件可执行。

- [ ] **Step 2: 写 drive_cc.sh**

```bash
cat > docs/superpowers/scripts/drive_cc.sh <<'EOF'
#!/usr/bin/env bash
# drive_cc.sh <label> <message> [answers_file]
#   以 CODECODER_ROOT=lab 跑 one-shot `cc "<message>"`,把 answers_file 内容管道喂给 stdin
#   (按序应答权限/ask/confirm/plan/trust 弹窗),stdout+stderr tee 到 lab/logs/<ts>-<label>.log。
set -uo pipefail
LABEL="${1:?label required}"; MSG="${2:?message required}"; ANSWERS="${3:-/dev/null}"
LAB="${CODECODER_ROOT:-/Users/rong.zhu/Code/codecoder-lab}"
CC_BIN="${CC_BIN:-/Users/rong.zhu/Code/codecoder/target/debug/cc}"
TS="$(date +%Y%m%d-%H%M%S)"
LOG="$LAB/logs/${TS}-${LABEL}.log"
mkdir -p "$LAB/logs"
{ echo "=== drive_cc $TS | $LABEL ==="; echo "MSG: $MSG"; } > "$LOG"
printf '%s\n' "$(<"$ANSWERS")" | CODECODER_ROOT="$LAB" "$CC_BIN" "$MSG" > "$LOG.body" 2>&1
RC=$?
cat "$LOG.body" | tee -a "$LOG"
echo "EXIT=$RC" | tee -a "$LOG"
rm -f "$LOG.body"
exit $RC
EOF
chmod +x docs/superpowers/scripts/drive_cc.sh
```
Expected: 文件可执行;`bash -n docs/superpowers/scripts/drive_cc.sh` 退出 0。

- [ ] **Step 3: 写 bg_runner.sh**

```bash
cat > docs/superpowers/scripts/bg_runner.sh <<'EOF'
#!/usr/bin/env bash
# bg_runner.sh <label> <task>  — CODECODER_BG_TASK headless,tee 日志,传播退出码。
set -uo pipefail
LABEL="${1:?label required}"; TASK="${2:?task required}"
LAB="${CODECODER_ROOT:-/Users/rong.zhu/Code/codecoder-lab}"
BG_BIN="${BG_BIN:-/Users/rong.zhu/Code/codecoder/target/debug/codecoder}"
TS="$(date +%Y%m%d-%H%M%S)"
LOG="$LAB/logs/${TS}-bg-${LABEL}.log"
mkdir -p "$LAB/logs"
{ echo "=== bg $TS | $LABEL ==="; echo "TASK: $TASK"; } > "$LOG"
CODECODER_ROOT="$LAB" CODECODER_BG_TASK="$TASK" "$BG_BIN" > "$LOG.body" 2>&1
RC=$?
cat "$LOG.body" | tee -a "$LOG"
echo "EXIT=$RC" | tee -a "$LOG"
rm -f "$LOG.body"
exit $RC
EOF
chmod +x docs/superpowers/scripts/bg_runner.sh
```
Expected: `bash -n` 退出 0。

- [ ] **Step 4: 自测 drive_cc.sh 机制(用 fake_cc,不烧 token)**

Run:
```bash
printf 'y\n' > /tmp/ans.txt
CC_BIN="$(pwd)/docs/superpowers/scripts/fake_cc.sh" \
  docs/superpowers/scripts/drive_cc.sh selftest "list files" /tmp/ans.txt
```
Expected: stdout 含 `⚙ list_directory:` 与 `  list_directory ✓` 与 `EXIT=0`;`codecoder-lab/logs/*-selftest.log` 存在且含同样内容。

- [ ] **Step 5: 自测断言**

Run: `grep -q 'list_directory ✓' codecoder-lab/logs/*-selftest.log && grep -q 'EXIT=0' codecoder-lab/logs/*-selftest.log && echo PASS`
Expected: `PASS`。

- [ ] **Step 6: 提交驱动脚本**

```bash
git add docs/superpowers/scripts/drive_cc.sh docs/superpowers/scripts/bg_runner.sh docs/superpowers/scripts/fake_cc.sh
git commit -m "chore: 新增 codecoder 探索驱动脚本

drive_cc.sh 用 piped-stdin 驱动 cc one-shot(prompt_user 读 io::stdin(),
按序喂 y/n 应答五类 Dialog);bg_runner.sh 驱动 CODECODER_BG_TASK。
规划期查明 pexpect 未装,此法更稳更简,且 fake_cc 桩可不烧 token 自测。"
```
Expected: commit 成功。

### Task 4: daemon + cc 端到端 smoke(真实 DeepSeek)

**Files:** 无新增;验证连通性。

- [ ] **Step 1: 起 daemon(后台)**

Run (background via harness): `CODECODER_ROOT=/Users/rong.zhu/Code/codecoder-lab set -a; source /Users/rong.zhu/Code/codecoder/.ccd.env; set +a; exec /Users/rong.zhu/Code/codecoder/target/debug/codecoder`
Expected: daemon 长驻;`codecoder-lab/.ccd.sock` 出现。

- [ ] **Step 2: cc 连接 + 一轮真实对话**

Run:
```bash
source /Users/rong.zhu/Code/codecoder/.ccd.env
docs/superpowers/scripts/drive_cc.sh smoke "用一句话介绍你自己,然后列出当前工作目录的文件" /dev/null
```
Expected: 日志含 codecoder 自述(StreamDelta 文本)+ `  list_directory ✓`;`EXIT=0`。

- [ ] **Step 3: smoke 断言 + 记矩阵**

Run:
```bash
grep -q 'list_directory ✓' codecoder-lab/logs/*-smoke.log && echo CONNECT_OK
printf '| cc↔daemon↔DeepSeek 连通 | works | drive_cc.sh smoke | %s | 真实 LLM 非罐头 |\n' "$(ls -t codecoder-lab/logs/*-smoke.log | head -1 | xargs basename)" >> codecoder-lab/matrix.md
```
Expected: `CONNECT_OK`;矩阵新增一行。

- [ ] **Step 4: 关 daemon**

Run: `CODECODER_ROOT=/Users/rong.zhu/Code/codecoder-lab /Users/rong.zhu/Code/codecoder/target/debug/cc shutdown`
Expected: daemon 退出(后续 Task 按需重启)。

---

## Phase 1 — 广度审计(交互式 cc 驱动)

> **统一流程**:每个 Task 起 daemon(Task 4 Step 1)→ `drive_cc.sh` 发指令(answers 按需)→ 用 `grep`/`jq` 断言日志与落盘产物 → 追加矩阵行 → 关 daemon。每个验证独立可复现。

### Task 5: 文件工具 + grep AST

- [ ] **Step 1: read_file / list_directory / glob**

Run: `drive_cc.sh file_read "读取 lab 的 AGENTS.md,并用 glob 列出 lab 下所有 .md 文件" /dev/null`
Expected: 日志含 `  read_file ✓`、`  glob ✓`;AGENTS.md 内容出现在 StreamDelta。

- [ ] **Step 2: grep 文本**

Run: `drive_cc.sh grep_text "用 grep 在 lab 下搜索 'Session' 这个词,报告命中的文件与行" /dev/null`
Expected: 日志含 `  grep ✓`;CONTEXT.md 的 Session 行命中。

- [ ] **Step 3: grep AST 查询(五语法)**

在 lab 放一个含函数定义的小 Rust/Python/JS/Go/C 样本,然后:
Run: `drive_cc.sh grep_ast "用 grep 的 AST 查询,找出 lab/samples/ 下的所有函数定义(rust/python/javascript/go/c 各试)" /dev/null`
Expected: 日志含多次 `  grep ✓`;命中各语法的函数节点。若某语法不受支持,如实标 limited。

- [ ] **Step 4: write_file / edit_file / diff**

Run: `drive_cc.sh file_write "在 lab 下新建文件 notes.md 写入一行 'hello',再用 edit_file 把 hello 改成 hi,最后用 diff 展示改动" /dev/null`
Expected: 日志含 `  write_file ✓`、`  edit_file ✓`、`  diff ✓`;`codecoder-lab/notes.md` 存在且内容为 hi。

- [ ] **Step 5: 断言 + 记矩阵(5 行)**

Run:
```bash
for t in read_file glob grep write_file edit_file diff; do grep -q "  $t ✓" codecoder-lab/logs/*-file_*.log codecoder-lab/logs/*-grep_*.log && echo "$t OK"; done
test "$(cat codecoder-lab/notes.md 2>/dev/null)" = "hi" && echo EDIT_OK
```
Expected: 各工具 `OK` + `EDIT_OK`。把 6 个工具的状态行追加到 `codecoder-lab/matrix.md`(AST 查询按实际五语法命中标注)。

### Task 6: 执行(run_command keying)

- [ ] **Step 1: 预授权命令直跑(git)**

Run: `drive_cc.sh run_git "在 lab 下运行 git init 并 git status,报告结果" /dev/null`
Expected: 日志含 `  run_command ✓`(因 codecoder.json 含 `run_command:git`);`codecoder-lab/.git/` 出现。

- [ ] **Step 2: 未预授权命令触发权限弹窗**

Run: `drive_cc.sh run_perm "在 lab 下运行 'uname -a' 并报告输出" <(printf 'y\n')`
Expected: 日志含 `🔐 Permission request: run_command:...`;管道喂 `y` 后 `  run_command ✓`。

- [ ] **Step 3: 断言 + 记矩阵**

Run: `grep -q '🔐 Permission request' codecoder-lab/logs/*-run_perm.log && echo PERM_PROMPT_OK`
Expected: `PERM_PROMPT_OK`。记两行矩阵(预授权 vs 弹窗)。

### Task 7: 自我进化闭环(Skill / Prompt / Capability Shell)

- [ ] **Step 1: use_skill 激活既有 skill**

把真实仓 `skills/self-verify.md` 复制进 `codecoder-lab/skills/` 作既有 skill;reload:
Run: `drive_cc.sh use_skill "激活 self-verify skill 并简述其要点" /dev/null`
Expected: 日志含 `  use_skill ✓`;skill 全文注入(StreamDelta 出现其内容)。

- [ ] **Step 2: generate_skill → /reload → use_skill 闭环**

Run: `drive_cc.sh gen_skill "撰写一个名为 lab-conventions 的 Skill(.md),内容是『lab 内提交前先跑 cargo test』,写入 skills/,然后激活它" /dev/null`
Expected: 日志含 `  generate_skill ✓`、`  use_skill ✓`;`codecoder-lab/skills/lab-conventions.md` 存在。

- [ ] **Step 3: generate_prompt → promote_prompt(草稿转正,ADR 0025)**

Run: `drive_cc.sh gen_prompt "撰写一个 Prompt 草稿名为 triage 写到 prompts/,内容是『分类 bug 优先级』;然后 promote_prompt 把它转正为 Skill(删草稿)" /dev/null`
Expected: 日志含 `  generate_prompt ✓`、`  promote_prompt ✓`;`codecoder-lab/prompts/triage.md` 不存在(已删)、`codecoder-lab/skills/triage.md` 存在。

- [ ] **Step 4: generate_capability + run_capability(Shell)**

Run: `drive_cc.sh gen_cap "撰写一个 Shell 环境的 OneShot Capability 名为 linter,manifest 声明 Environment=Shell/Lifecycle=OneShot,功能是跑 'echo lint-ok';然后 run_capability 执行它" /dev/null`
Expected: 日志含 `  generate_capability ✓`、`  run_capability ✓`;`codecoder-lab/capabilities/linter/` 存在(含 manifest);输出含 `lint-ok`。

- [ ] **Step 5: 断言 + 记矩阵(4 行)**

Run:
```bash
test -f codecoder-lab/skills/lab-conventions.md && echo GEN_SKILL_OK
test -f codecoder-lab/skills/triage.md && test ! -e codecoder-lab/prompts/triage.md && echo PROMOTE_OK
test -d codecoder-lab/capabilities/linter && echo GEN_CAP_OK
```
Expected: 三个 OK。记矩阵 4 行(use_skill/generate_skill 闭环/promote_prompt/generate_capability+run_capability Shell)。

### Task 8: Wasm capability + 已知未实现核验(Wasm 源码编译 / Docker 不降级)

- [ ] **Step 1: 预置一个最小 .wat,跑 Wasm capability**

Run: `drive_cc.sh wasm_cap "撰写一个 Wasm 环境 OneShot Capability 名为 mdcount,执行一个预编译的 .wat(功能:返回数字 42);然后 run_capability 在 Wasm 环境执行它" /dev/null`
Expected: 日志含 `  run_capability ✓`;输出含 `42`。记 `works`。

- [ ] **Step 2: 核验「Wasm 源码→wasm 编译未实现」(ADR 0021)**

Run: `drive_cc.sh wasm_src "写一个 Wasm capability 但 Environment 声明的入口指向一段 Rust 源码(非 .wasm/.wat),然后 run_capability,报告是否被接受" /dev/null`
Expected: 日志含 `  run_capability ✗`(或 Notice 点明只接受预编译);不接受源码。记 `unimplemented`。

- [ ] **Step 3: 核验「Docker 缺失显式报错不降级」(ADR 0021)**

Run: `drive_cc.sh docker_nodegrade "声明一个 Docker 环境 Capability 然后 run_capability;本机大概率无 Docker,验证它显式报错而不是偷偷落到 Shell" /dev/null`
Expected: 日志含 `  run_capability ✗` 且错误信息点明 Docker 不可用(无降级迹象)。记 `works`(契约生效)或 `limited`(若实际降级,记为发现)。

- [ ] **Step 4: 断言 + 记矩阵(3 行)**

把三步结论各记一行(`works`/`unimplemented`/契约生效)。

### Task 9: 委派/交互(agent / review / ask_user / confirm — 五类 Dialog)

- [ ] **Step 1: agent 子 agent(只读子集 + 深度锁 1)**

Run: `drive_cc.sh sub_agent "用 agent 工具派一个子 agent 去只读勘察 lab 的 skills/ 目录并汇报内容" /dev/null`
Expected: 日志含 `  agent ✓`;子 agent 汇报内容;子 agent 只用 `Permission::None` 工具(观察 StreamDelta 是否仅 read/list/glob/grep)。

- [ ] **Step 2: review 结构化 Verdict + 四信号**

在 lab 造一段「故意过度设计」的小代码,然后:
Run: `drive_cc.sh review_tool "对 lab/samples/over_eng.rs 跑 review,产出结构化 Verdict(pass/needs_fix/rebuild)与四信号(foundation/over_engineering/volume/terminology)" /dev/null`
Expected: 日志含 `  review ✓`;StreamDelta/产物含 Verdict 字段与四信号。

- [ ] **Step 3: ask_user(自由文本 Dialog)**

Run: `drive_cc.sh ask_user "用 ask_user 问我『想要哪个语言』,然后基于我的回答写一个 hello 文件" <(printf 'Rust\n')`
Expected: 日志含 `> `(ask_user prompt)+ 管道喂 `Rust`;后续基于回答行动。

- [ ] **Step 4: confirm(yes/no Dialog)**

Run: `drive_cc.sh confirm "用 confirm 问我『是否创建 demo 文件』,yes 则创建" <(printf 'y\n')`
Expected: 日志含 `[y/n]:`;喂 `y` 后创建文件。

- [ ] **Step 5: trust Dialog(ADR 0028,在干净 trust 状态下)**

清空 `~/.codecoder/trust.json` 中 lab 条目,设 `CODECODER_DEFAULT_TRUST` 未设;起 daemon 时 codecoder 应发 Trust 弹窗。
Run: `drive_cc.sh trust_gate "一句话自述" <(printf 'o\n')`
Expected: 日志含 `Trust this project's disk self?` + `[a]lways / [o]nce / [n]ever:`;喂 `o` 后加载 lab 自我。

- [ ] **Step 6: 断言 + 记矩阵(5 行)**

Run:
```bash
grep -q '  agent ✓' codecoder-lab/logs/*-sub_agent.log && echo AGENT_OK
grep -q '  review ✓' codecoder-lab/logs/*-review_tool.log && echo REVIEW_OK
grep -q '\[y/n\]' codecoder-lab/logs/*-confirm.log && echo CONFIRM_OK
grep -q 'Trust this project' codecoder-lab/logs/*-trust_gate.log && echo TRUST_OK
```
Expected: 四个 OK。记矩阵 5 行(agent/review/ask_user/confirm/trust)。

### Task 10: 联网(search_web / search_github / reverse_api)

- [ ] **Step 1: search_web**

Run: `drive_cc.sh web "用 search_web 抓取 https://example.com 并报告页面标题/首行" /dev/null`
Expected: 日志含 `  search_web ✓`;输出含 example.com 内容。若沙箱不通,日志含 `  search_web ✗` → 记 `limited`。

- [ ] **Step 2: search_github(repos: 与 code:)**

Run: `drive_cc.sh gh "用 search_github 搜 repos: 'tree-sitter rust grammar' 与 code: 'fn default_sock_path'" /dev/null`
Expected: 日志含 `  search_github ✓`;两类结果出现。无 `GITHUB_TOKEN` 可能限流 → 记 `limited`。

- [ ] **Step 3: reverse_api**

Run: `drive_cc.sh revapi "用 reverse_api 抓取一个公开文档页(如 https://doc.rust-lang.org/std/vec/struct.Vec.html)并提取其公开方法签名" /dev/null`
Expected: 日志含 `  reverse_api ✓`;输出含方法签名。不通则记 `limited`。

- [ ] **Step 4: 断言 + 记矩阵(3 行,按实际连通性标 works/limited)**

### Task 11: 开发(diff / commit)

- [ ] **Step 1: commit(git,真实生效)**

在 lab 已 `git init`(Task 6)前提下:
Run: `drive_cc.sh commit_tool "在 lab 加一个 README.md 写一行说明,然后用 commit 工具提交" /dev/null`
Expected: 日志含 `  commit ✓`;`git -C codecoder-lab log --oneline` 出现一条提交。

- [ ] **Step 2: 断言 + 记矩阵(1 行)**

Run: `git -C codecoder-lab log --oneline | head -1 | grep -q . && echo COMMIT_OK`
Expected: `COMMIT_OK`。

### Task 12: 规划/推理(plan / milestone / memory / reason)

- [ ] **Step 1: plan 工具**

Run: `drive_cc.sh plan_tool "用 plan 工具为『给 lab 加一个 todo 清单功能』给出任务计划" /dev/null`
Expected: 日志含 PlanApproval Dialog 或 `  plan ✓`。

- [ ] **Step 2: milestone workgraph 七动作 + drive_workgraph 自动推进**

Run: `drive_cc.sh workgraph "用 milestone 工具:list 现状;add 三个依赖有序里程碑 A→B→C;start A;next 看就绪;done A 后再看 next;最后 needs_fix B 再 remove C。完整走一遍" /dev/null`
Expected: 日志多次 `  milestone ✓`;`codecoder-lab/workgraph.json` 存在且 `jq '.milestones|length' codecoder-lab/workgraph.json` ≥3;跨 turn 后 `drive_workgraph` 自动推进(Task 15 深度验证,此处验文件落盘)。

- [ ] **Step 3: memory 跨 session KV**

Run: `drive_cc.sh mem_set "用 memory 工具写入 key=lab-fact value='mdslides 目标是 markdown→slides'" /dev/null`
然后新起一个 cc session:
Run: `drive_cc.sh mem_get "用 memory 工具读取 key=lab-fact 并复述" /dev/null`
Expected: 两次 `  memory ✓`;`codecoder-lab/memory/lab-fact` 存在;第二次复述出原值(证明跨 session)。

- [ ] **Step 4: reason causal tree + 跨 session meta 检索**

Run: `drive_cc.sh reason_add "用 reason 工具:add 一个根因节点『mdslides 测试失败』,带 margin/leverage;再 add 一个子节点『因 Cargo.toml 缺 dev-deps』;status 看树;list 看全部" /dev/null`
Expected: 日志多次 `  reason ✓`;`codecoder-lab/causal_tree.json` 存在且 `jq` 可见节点;cross action 检索跨 session meta(验日志或产物)。

- [ ] **Step 5: 断言 + 记矩阵(4 行)**

Run:
```bash
test -f codecoder-lab/workgraph.json && echo WG_OK
test -f codecoder-lab/memory/lab-fact && echo MEM_OK
test -f codecoder-lab/causal_tree.json && echo REASON_OK
```
Expected: 三个 OK。记矩阵 4 行。

### Task 13: 横切(权限 scope / trust / compaction / session 树 / wire)

- [ ] **Step 1: Permission Scope(Once/Session/Project)**

Run: `drive_cc.sh perm_scope "运行一个未预授权命令,弹窗时我会选 session-always,然后再次运行同命令验证不再弹窗" <(printf 's\n')`
Expected: 日志含 `🔐 Permission request` + 喂 `s`;第二次同命令日志不再含弹窗(SessionAllowlist 生效)。

- [ ] **Step 2: trust 门禁(ADR 0028,never 不加载自我)**

设 `CODECODER_DEFAULT_TRUST=never`,清 trust 条目,起 daemon:
Run: `drive_cc.sh trust_never "列出当前目录文件" /dev/null`(env 带 `CODECODER_DEFAULT_TRUST=never`)
Expected: codecoder 不加载 lab AGENTS.md/skills(行为退化为无自我),或显式拒绝;记观察。

- [ ] **Step 3: compaction tier-1 + tier-2(造超长上下文)**

连续灌入大量内容(如反复 read 大文件)直到 `ctx%` 接近上限:
Run: 多次 `drive_cc.sh compact_N "读取 lab/samples/big.txt 并逐段总结" /dev/null`
Expected: 日志 stderr 出现 `[ctx <高>%]`;触发 tier-1(丢 Reasoning + 占位化旧 ToolResult)后 ctx 回落;持续超限触发 tier-2(LLM 摘要为合成 System 消息)。验日志含 compaction 迹象。

- [ ] **Step 4: session 树 + /resume**

Run: `drive_cc.sh sess_tree "做点小事" /dev/null`;然后 `cc sessions` 列出;`cc clone` / `cc fork <id>` 验证树状(parent/leaf)。
Expected: `codecoder-lab/sessions/` 出现新 JSON;`cc sessions` 列出 id;tree 结构可见。

- [ ] **Step 5: wire 往返汇总**

汇总前述 Task 中所有 `🔐`/`[y/n]`/`>`/`Approve?`/`Trust` 弹窗均经 daemon wire 往返且 cc 行内应答成功 → 记矩阵一行「client-server wire 五类 Dialog」works。

- [ ] **Step 6: 断言 + 记矩阵(5 行)**

记:Permission Scope / trust 门禁 / compaction / session 树 / wire 往返。

---

## Phase 2 — 深度展示(mdslides 集成任务)

### Task 14: 种子 mdslides + workgraph 里程碑计划

**Files:**
- Create: `codecoder-lab/showcase/mdslides/`(由 codecoder 在本任务创建初始骨架)
- Modify: `codecoder-lab/workgraph.json`(由 codecoder milestone add 填充)

- [ ] **Step 1: 种子任务说明文件**

Run:
```bash
mkdir -p codecoder-lab/showcase
cat > codecoder-lab/showcase/TASK.md <<'EOF'
# 任务:端到端造 mdslides Rust crate

在 showcase/mdslides/ 造一个小 Rust crate:把 markdown 转成 slides(JSON 或 HTML),带单测。
要求:用 milestone 工具先拆依赖有序里程碑(parser→slide model→renderer→tests→docs),
跨 turn 让 drive_workgraph 自动推进;为 renderer 沉淀一个 Skill 并 use_skill;
写一个 Shell capability 做 lint/build、一个 Wasm capability 做纯计算核(slide 计数);
用 review 验收产物;遇阻用 reason 建因果树;关键事实存 memory。
EOF
```
Expected: 文件写入。

- [ ] **Step 2: 起 daemon + 触发 workgraph 规划(交互式 cc)**

Run: `drive_cc.sh mdslides_plan "阅读 showcase/TASK.md,用 milestone 工具拆出依赖有序的里程碑图,然后用 plan 给执行计划" <(printf 'y\n')`
Expected: 日志含多次 `  milestone ✓`、`  plan ✓`;`workgraph.json` 含 ≥4 里程碑、依赖有序。

- [ ] **Step 3: 断言 workgraph 落盘**

Run: `jq '.milestones|length' codecoder-lab/workgraph.json` 与 `jq '.milestones[].deps // []' codecoder-lab/workgraph.json`
Expected: 里程碑数 ≥4;deps 体现依赖序。

### Task 15: headless BG 跑 showcase + 中途 SIGINT + 重跑至完成

**Files:** 产物由 codecoder 写入 `codecoder-lab/showcase/mdslides/`、`skills/`、`capabilities/`、`memory/`、`causal_tree.json`。

- [ ] **Step 1: 用 bg_runner headless 启动 showcase(后台)**

Run (background): `docs/superpowers/scripts/bg_runner.sh mdslides_run "执行 showcase/TASK.md:推进 workgraph 里程碑,造 mdslides crate,写 skill 与两个 capability,review 验收,遇阻 reason,关键事实存 memory"`
Expected: 后台进程启动,日志持续增长。

- [ ] **Step 2: 中途发 SIGINT,验证优雅取消**

等 ~15s(让 turn 进入 run_command/run_capability 等可取消点),取后台进程 PID:
Run: `pkill -INT -f 'CODECODER_BG_TASK.*mdslides_run'`(或按 harness 给出的 PID `kill -INT <pid>`)
Expected: bg_runner 日志出现取消迹象:`EXIT≠0` 或 Notice/`cancel`/CancelToken 相关;`BgOutcome.denied` 或被中断的 step 可见;子进程被 kill(无残留 `cargo`/capability 子进程)。

- [ ] **Step 3: 断言取消生效**

Run:
```bash
grep -qiE 'cancel|interrupt|SIGINT' codecoder-lab/logs/*-bg-mdslides_run.log && echo CANCEL_OK
pgrep -f 'CODECODER_BG_TASK' && echo STILL_RUNNING || echo NO_RESIDUAL
```
Expected: `CANCEL_OK` + `NO_RESIDUAL`(无残留进程)。

- [ ] **Step 4: 重跑至完成**

Run: `docs/superpowers/scripts/bg_runner.sh mdslides_rerun "继续推进 showcase/TASK.md 到完成:跑通 mdslides crate 的单测"`
Expected: `EXIT=0`;日志含 `  commit ✓` 或单测通过迹象;`codecoder-lab/showcase/mdslides/` 含 Cargo.toml + src + tests。

- [ ] **Step 5: 断言 crate 跑通**

Run: `test -f codecoder-lab/showcase/mdslides/Cargo.toml && (cd codecoder-lab/showcase/mdslides && cargo test 2>&1 | tail -3)`
Expected: Cargo.toml 存在;`cargo test` 在该 crate 内通过(或如实记录失败)。

### Task 16: 捕获 showcase 产物 + 验证各硬特性确实被用

- [ ] **Step 1: 汇总落盘产物**

Run:
```bash
echo "skills:"; ls codecoder-lab/skills/
echo "capabilities:"; ls codecoder-lab/capabilities/ 2>/dev/null
echo "memory:"; ls codecoder-lab/memory/ 2>/dev/null
echo "causal:"; jq '.nodes|length' codecoder-lab/causal_tree.json 2>/dev/null
echo "workgraph milestones:"; jq '.milestones|length' codecoder-lab/workgraph.json 2>/dev/null
echo "wg done:"; jq '[.milestones[]|select(.status=="Done")] | length' codecoder-lab/workgraph.json 2>/dev/null
```
Expected: 各产物存在;workgraph 有 Done 里程碑;causal 有节点(若 showcase 顺利无阻,reason 可能未被触发 → 如实记 limited)。

- [ ] **Step 2: 验证 Wasm + Shell capability 均被执行过**

Run: `grep -h 'run_capability ✓' codecoder-lab/logs/*-bg-mdslides_*.log | sort -u`
Expected: 至少命中 Shell 与 Wasm 两类(若 Wasm 因源码编译未实现而用预编译 .wat,记备注)。

- [ ] **Step 3: 验证 review 产出 Verdict**

Run: `grep -q '  review ✓' codecoder-lab/logs/*-bg-mdslides_*.log && echo REVIEW_RAN`
Expected: `REVIEW_RAN`(或如实记未触发)。

- [ ] **Step 4: 记矩阵(showcase 行)**

把 mdslides 集成任务中各硬特性的实际命中情况记入矩阵(works/limited + 证据日志名)。

---

## Phase 3 — 综合

### Task 17: 写能力矩阵 + 上限报告 + 提交

**Files:**
- Create: `docs/superpowers/audits/2026-07-21-codecoder-capability-matrix.md`

- [ ] **Step 1: 汇总 lab/matrix.md + 各 Task 证据**

把 `codecoder-lab/matrix.md` 草稿、文档计数核验、已知未实现核验、showcase 命中情况,整理成最终报告。结构:
1. 总览(探索范围、方法、环境)。
2. 能力矩阵(全部能力 → 状态 → 证据 → 备注)。
3. 「已知未实现」核验结果(Wasm 源码编译 / Persistent 跨重启 / 内置调度器 / margin-leverage 仅元数据)。
4. 文档计数核对(测试数 / 26 工具 / 23 ADR 与实际是否一致)。
5. 上限在哪 / 可突破点(基于压测观察)。

- [ ] **Step 2: 写报告文件**

把 Step 1 内容写入 `docs/superpowers/audits/2026-07-21-codecoder-capability-matrix.md`。

- [ ] **Step 3: 报告自检(无占位符、状态与证据一致)**

Run: `grep -nE 'TBD|TODO|待补|占位' docs/superpowers/audits/2026-07-21-codecoder-capability-matrix.md`
Expected: 无命中(或均为引用「已知未实现」的正当说明)。

- [ ] **Step 4: 提交报告**

```bash
git add docs/superpowers/audits/2026-07-21-codecoder-capability-matrix.md docs/superpowers/specs/2026-07-21-codecoder-capability-exploration-design.md docs/superpowers/plans/2026-07-21-codecoder-capability-exploration.md
git commit -m "docs: 新增 codecoder 能力矩阵与上限压测报告

基于广度审计(全部内置能力)+ 深度展示(mdslides 集成任务压测 workgraph/
自我进化/Wasm+Shell capability/review/reason/SIGINT 取消)得出真实能力
上限与已知缺口;附带文档计数核对(测试数文档不一致已记为发现)。"
```
Expected: commit 成功在 `explore/codecoder-capability-audit` 分支。

---

## Self-Review(plan vs spec)

**1. Spec coverage:**
- Phase 0(编译/测试/lab/驱动/smoke)→ Task 1–4 ✓
- Phase 1 广度审计全部类别(文件/执行/自我进化/Wasm+Docker/委派交互/联网/开发/规划推理/横切)→ Task 5–13 ✓
- 「已知未实现」核验(Wasm 源码/Persistent/调度器/margin 元数据)→ Task 8 + 报告 Task 17 ✓(Persistent 跨重启与调度器上限属延后项,在报告里基于源码+ADR 标注;本计划不实跑 daemon 重启持久化测试以省成本,如实记为「源码核验」)
- Phase 2 深度展示(mdslides + workgraph + 自我进化 + Wasm/Shell cap + review/reason/memory + BG/SIGINT)→ Task 14–16 ✓
- Phase 3 报告 + 文档计数 → Task 17 ✓
- 双模式(交互 cc + headless BG)→ Phase 1 用 drive_cc.sh,Phase 2 用 bg_runner.sh ✓
- 隔离工作区 → 全程 `CODECODER_ROOT=codecoder-lab` ✓

**2. Placeholder scan:** 无 TBD/TODO;LLM 非确定性步骤均用结构化断言(文件存在/jq/grep 标记/退出码)而非猜测字符串 ✓

**3. Type consistency:** drive_cc.sh/bg_runner.sh 接口在 Task 3 定义,后续 Task 统一用 `<label> <msg> [answers]` / `<label> <task>` ✓;矩阵行格式全局统一 ✓
