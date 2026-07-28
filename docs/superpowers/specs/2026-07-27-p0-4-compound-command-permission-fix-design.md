# P0-4: 复合命令权限修复 — 设计文档

> 7×24 高度自主开发差距 P0-4：复合命令（含 `2>&1`/`|`/`&&` 等）的 permission key 变成完整命令串，不可经首 token 预授权。修复方式：headless 模式下对复合命令也按首 token keying，并增加安全护栏。

---

## 现状

`src/tool/builtin.rs` — `RunCommand::key_for()`（第 71-78 行）：

```rust
fn key_for(cmd: &str) -> String {
    if Self::is_compound(cmd) {
        format!("run_command:{cmd}")
    } else {
        let head = cmd.split_whitespace().next().unwrap_or("");
        format!("run_command:{head}")
    }
}
```

复合命令 → `run_command:<完整命令串>`，不可预授权。headless 模式下被 denied。

## 设计

### 方案

在 `key_for()` 中增加 `headless` 参数。当 `headless == true` 时，复合命令也提取首 token 作为 key：
- `npm run build 2>&1` → `run_command:npm`
- `find . -name "*.ts" | sort` → `run_command:find`

交互模式下保持完整 key，安全不变。

### 安全护栏

宽松模式仅在 `headless && CODECODER_DEFAULT_TRUST=always` 时生效。这是因为：
- headless 模式已有 `Self::AllRunCommandsAllowed` 级别的大权限模式（交互下不可用）
- `CODECODER_DEFAULT_TRUST=always` 表示用户显式信任该项目

### 修改点

| 文件 | 变更 | 类型 |
|------|------|------|
| `src/tool/builtin.rs` | `key_for()` 增加 `headless` 参数 | 修改 |
| `src/tool/builtin.rs` | `permission()` 调用 `key_for()` 时传入 headless | 修改 |
| `src/tool/builtin.rs` | key_for 测试适配 headless 参数 | 修改 |

### 具体代码变更

```rust
/// Permission key(ADR 0018)。简单命令按命令类(`run_command:git`);
/// 复合命令在 headless+trusted 模式下也按命令类(**同前缀预授权**)，否则整条命令串。
fn key_for(cmd: &str, headless: bool) -> String {
    let head = cmd.split_whitespace().next().unwrap_or("");
    if Self::is_compound(cmd) && !(headless && trust::should_default_to_trusted()) {
        format!("run_command:{cmd}")
    } else {
        format!("run_command:{head}")
    }
}
```

`permission()` 调用处：
```rust
fn permission(&self, args: &Value, _root: &Path) -> Permission {
    let cmd = args.get("cmd").and_then(Value::as_str).unwrap_or_default();
    // 注入 headless 信息：Permission 函数本身无 headless 上下文，但
    // 可以通过检测环境变量判断（dispatch_tool 已有此信息但未传递）。
    // 改用模块级 flag 或直接从环境读取 trust 配置。
}
```

**一个接口问题：** `permission()` 不知道 `headless` 状态，因为 `Tool::permission()` 签名是 `(&self, args, root)`，没有 headless 参数。解决方案：运行时直接从 `Config` 读取 `CODECODER_DEFAULT_TRUST` 状态，如果为 `always` 则放宽 keying。

```rust
fn key_for(cmd: &str) -> String {
    let head = cmd.split_whitespace().next().unwrap_or("");
    if Self::is_compound(cmd) && !trust::should_default_to_trusted() {
        format!("run_command:{cmd}")
    } else {
        format!("run_command:{head}")
    }
}
```

`trust::should_default_to_trusted()` 检查 `CODECODER_DEFAULT_TRUST=always`。

### 验收标准

1. `npm run build 2>&1` 在 `CODECODER_DEFAULT_TRUST=always` 下 key 为 `run_command:npm`
2. `npm run build 2>&1` 在 `CODECODER_DEFAULT_TRUST` 未设置时 key 为完整命令串
3. 简单命令（`git status`）行为不变，key 为 `run_command:git`
4. 测试覆盖复合命令 + trusted/not trusted 两种场景
