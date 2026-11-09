#!/bin/sh
# Drives the UI in headless Chromium with a fake backend: runs the behaviour checks, then
# takes screenshots of every scenario in light and dark at desktop width.
# Usage: app/ui-tests/run.sh [output directory]   (default: app/target/ui-screenshots)
set -eu

APP_DIR=$(cd "$(dirname "$0")/.." && pwd)
OUT=${1:-"$APP_DIR/target/ui-screenshots"}
CHROMIUM=${CHROMIUM:-chromium}
SIZE=1280,800
SCENARIOS="empty port-error no-tabs list focused ask ended disconnected sessions menu error"

READY_ATTEMPTS=50

mkdir -p "$OUT"
# A free port: bind to port 0 and read what the system chose.
PORT=$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
python3 -m http.server "$PORT" --bind 127.0.0.1 --directory "$APP_DIR" >/dev/null 2>&1 &
SERVER=$!
trap 'kill $SERVER' EXIT
attempt=0
until curl -sf "http://127.0.0.1:$PORT/ui-tests/harness.html" >/dev/null; do
  attempt=$((attempt + 1))
  if [ "$attempt" -ge "$READY_ATTEMPTS" ]; then
    echo "the test server did not start" >&2
    exit 1
  fi
  sleep 0.1
done

browse() {
  "$CHROMIUM" --headless --no-sandbox --disable-gpu --hide-scrollbars --window-size=$SIZE \
    --virtual-time-budget=3000 "$@"
}

RESULT=$(browse --dump-dom "http://127.0.0.1:$PORT/ui-tests/harness.html?scenario=checks" 2>/dev/null \
  | sed -n 's:.*<output id="result" hidden="">\(.*\)</output>.*:\1:p')
echo "checks: ${RESULT:-no result}"

for scenario in $SCENARIOS; do
  url="http://127.0.0.1:$PORT/ui-tests/harness.html?scenario=$scenario"
  browse --screenshot="$OUT/$scenario-light.png" "$url" 2>/dev/null
  browse --force-dark-mode --blink-settings=preferredColorScheme=0 \
    --screenshot="$OUT/$scenario-dark.png" "$url" 2>/dev/null
done
echo "screenshots: $OUT"
[ "$RESULT" = "PASS" ]
