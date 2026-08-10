#!/usr/bin/env bash
set -euo pipefail

MODE="${1:-run}"
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_NAME="屿阅"

workspace_pids() {
  ps -axo pid=,command= | awk '
    /(^|[[:space:]\/])src-tauri\/target\/(debug|release)\/tauri-app([[:space:]]|$)/ { print $1 }
  '
}

stop_workspace_apps() {
  while read -r pid; do
    if [[ -n "$pid" && "$pid" != "$$" ]]; then
      kill "$pid" >/dev/null 2>&1 || true
    fi
  done < <(workspace_pids)
}

cd "$ROOT_DIR"
pkill -x "$APP_NAME" >/dev/null 2>&1 || true
stop_workspace_apps

start_dev() {
  npm run tauri dev
}

case "$MODE" in
  run)
    start_dev
    ;;
  --debug|debug)
    RUST_BACKTRACE=1 RUST_LOG=debug start_dev
    ;;
  --logs|logs)
    start_dev >"$ROOT_DIR/.codex/mdreader-dev.log" 2>&1 &
    dev_pid=$!
    trap 'kill "$dev_pid" >/dev/null 2>&1 || true' EXIT
    sleep 8
    /usr/bin/log stream --info --style compact --predicate "process == \"$APP_NAME\""
    ;;
  --telemetry|telemetry)
    echo "屿阅 v0.1 不包含遥测；以下仅启动本地开发运行，不收集或发送 telemetry。" >&2
    start_dev
    ;;
  --verify|verify)
    start_dev >"$ROOT_DIR/.codex/mdreader-dev.log" 2>&1 &
    dev_pid=$!
    trap 'kill "$dev_pid" >/dev/null 2>&1 || true' EXIT
    for _ in {1..30}; do
      if [[ -n "$(workspace_pids)" ]]; then
        echo "$APP_NAME is running"
        exit 0
      fi
      sleep 1
    done
    echo "$APP_NAME did not start" >&2
    exit 1
    ;;
  *)
    echo "usage: $0 [run|--debug|--logs|--telemetry|--verify]" >&2
    exit 2
    ;;
esac
