#!/usr/bin/env bash
# fake_cc.sh — drive_cc.sh 自测桩:模拟 cc one-shot。
#   读 stdin 一行作 prompt 应答,打印与真 cc 相同格式的事件标记,TurnComplete 退出。
#   用途:不烧 LLM token 即可验证 drive_cc.sh 的管道/日志/退出码机制。
read -r ans || true
echo "⚙ list_directory: ."
echo "  list_directory ✓"
echo "(turn complete)"
exit 0
