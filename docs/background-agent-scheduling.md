# Background Agent 调度方案

CodeCoder 的 Background Agent 是一个 headless one-shot runner（见 ADR 0026 和 `src/background.rs`）。调度器不在代码中，由外部工具负责。以下为三个推荐方案。

## 方案 A: systemd timer（Linux 首选）

1. 创建 service unit `/etc/systemd/system/codecoder-bg.service`:

```ini
[Unit]
Description=CodeCoder Background Agent
After=network.target

[Service]
Type=oneshot
Environment=CODECODER_BG_TASK=workgraph
Environment=CODECODER_ROOT=/path/to/project
Environment=CODECODER_API_KEY=sk-...
ExecStart=/usr/local/bin/codecoder --background
User=your-user
```

2. 创建 timer unit `/etc/systemd/system/codecoder-bg.timer`:

```ini
[Unit]
Description=Run CodeCoder Background Agent every 15 minutes

[Timer]
OnCalendar=*:0/15
Persistent=true

[Install]
WantedBy=timers.target
```

3. 启用:

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now codecoder-bg.timer
```

## 方案 B: cron（跨平台）

```bash
# 每 30 分钟运行一次
*/30 * * * * cd /path/to/project && CODECODER_BG_TASK=workgraph CODECODER_ROOT=/path/to/project CODECODER_API_KEY=sk-... /usr/local/bin/codecoder --background >> /var/log/codecoder-bg.log 2>&1
```

## 方案 C: launchd（macOS）

创建 `~/Library/LaunchAgents/com.codecoder.bg.plist`:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.codecoder.bg</string>
    <key>ProgramArguments</key>
    <array>
        <string>/usr/local/bin/codecoder</string>
        <string>--background</string>
    </array>
    <key>EnvironmentVariables</key>
    <dict>
        <key>CODECODER_BG_TASK</key>
        <string>workgraph</string>
        <key>CODECODER_ROOT</key>
        <string>/path/to/project</string>
    </dict>
    <key>StartInterval</key>
    <integer>900</integer>
    <key>RunAtLoad</key>
    <true/>
</dict>
</plist>
```

```bash
launchctl load ~/Library/LaunchAgents/com.codecoder.bg.plist
```

## 关键配置

- `CODECODER_BG_TASK=workgraph` — 自动推进 Work Graph 下一个 ready milestone（最多 3 个）
- `CODECODER_BG_TASK=<自定义任务描述>` — 运行固定文本任务
- `CODECODER_ROOT` — 项目根目录（必需）
- `CODECODER_API_KEY` — LLM API key（必需）
- `CODECODER_MODEL` — 模型名称（默认 `gpt-4o`）

## 日志

Background Agent 默认输出到 stdout/stderr。在调度器中重定向到文件:

```bash
codecoder --background >> /var/log/codecoder-$(date +%Y%m%d).log 2>&1
```

## 延后项

- 内置调度器（进程内定时器）
- 多 runner 资源上限
- SIGINT 优雅关闭
