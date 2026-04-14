#!/bin/bash
set -e

# 启动 trace 和 serve
agentsight trace &
TRACE_PID=$!

agentsight serve --host 0.0.0.0&
SERVE_PID=$!

# 捕获信号，优雅退出
trap 'kill $TRACE_PID $SERVE_PID 2>/dev/null; exit 0' SIGTERM SIGINT SIGHUP

# 等待任意进程退出
wait -n
exit_code=$?

# 一个进程退出，关闭另一个
kill $TRACE_PID $SERVE_PID 2>/dev/null || true
exit $exit_code
