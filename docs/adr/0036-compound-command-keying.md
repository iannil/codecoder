# ADR 0036 — 复合命令 keying 加固

- **状态**: Accepted
- **日期**: 2026-07-22
- **关联**: ADR 0018(Tool trait 与 PermissionKey)、上限压测 P5(`docs/superpowers/audits/2026-07-22-codecoder-ceiling-probe.md`)

## 背景

P5 发现:`run_command` 的 PermissionKey 取首空白 token(`split_whitespace().next()`),而命令经 `sh -c` 整串执行。故 `cd X && rm -rf Y` 只被 `run_command:cd` 门控——**预授权良性前缀(`cd`/`ls`/`echo`)隐式授权后缀任意命令**。

## 决策

1. **复合命令按整条命令串 keying**:`run_command:<整 cmd>`,几乎不可经前缀预授权 → 交互每次弹(显全文)、headless 拒绝。简单命令(无运算符)保持首 token keying(`run_command:git`),既有可预授权性不变。
2. **保守检测偏向安全**:`is_compound` 命中 `&&`/`||`/`;`/`|`/backtick/`$(`/`<`/`>` 或末尾 `&` 即判复合。引号内的 `>`/`<` 会误判(过度弹窗),但**绝不漏判**——安全优先于 UX。
3. **执行路径不变**:仍 `sh -c <整 cmd>`,复合命令仍能跑,只是权限 key 变。

## 后果

- **正面**:堵住"良性前缀→任意后缀"漏洞;用户审批复合命令时看到整条命令全文。
- **代价**:合法复合(`cd X && cargo test`)交互下每次弹(不再能经 `run_command:cargo` 预授权);headless 下复合被拒,agent 须自适应(改 `cargo test --manifest-path`,报告已观察此行为)。引号内含 `>`/`<` 的简单命令被误判为复合(过度弹窗)。
- **不做**:逐子命令授权(shell 拆分雷区);精确 shell 解析识别引号;复合命令通配 allowlist。
