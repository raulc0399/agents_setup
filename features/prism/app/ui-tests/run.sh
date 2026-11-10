#!/bin/sh
# Drives the UI in headless Chromium with a fake backend: runs the behaviour checks, then
# takes screenshots of every scenario in light and dark at desktop width. Diagrams are laid
# out by prism-core through the `diagram` example of prism-app-host, built first.
# Usage: app/ui-tests/run.sh [output directory]   (default: app/target/ui-screenshots)
set -eu

APP_DIR=$(cd "$(dirname "$0")/.." && pwd)
OUT=${1:-"$APP_DIR/target/ui-screenshots"}
CHROMIUM=${CHROMIUM:-chromium}
SIZE=1280,800
CHECKS="checks diagram-checks action-checks status-checks"
SCENARIOS="empty port-error mismatch no-tabs list focused ask select questions filter accepted ended disconnected sessions menu error diagram diagram-panel diagram-zoomed diagram-ask"
# A narrow window: the diagram's panel goes below the canvas.
NARROW_SIZE=600,760
NARROW_CHECKS="narrow-checks"
NARROW_SCENARIOS="diagram-panel"

READY_ATTEMPTS=50
# Virtual milliseconds a page may run: a screenshot settles quickly, the checks replay a lot.
SCREENSHOT_BUDGET_MS=3000
CHECK_BUDGET_MS=15000

mkdir -p "$OUT"
(cd "$APP_DIR" && CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-2} cargo build --quiet --locked -p prism-app-host --example diagram)
# Wherever cargo builds (CARGO_TARGET_DIR, a config file): it says so in its metadata.
TARGET_DIR=$(cd "$APP_DIR" && cargo metadata --format-version 1 --no-deps --locked \
  | python3 -c 'import json, sys; print(json.load(sys.stdin)["target_directory"])')
DIAGRAM="$TARGET_DIR/debug/examples/diagram"
# A free port: bind to port 0 and read what the system chose.
PORT=$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
python3 "$APP_DIR/ui-tests/server.py" "$PORT" "$APP_DIR" "$DIAGRAM" >/dev/null 2>&1 &
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

# browse <window size> <virtual time budget> <chromium arguments>
browse() {
  size=$1
  budget=$2
  shift 2
  "$CHROMIUM" --headless --no-sandbox --disable-gpu --hide-scrollbars --window-size="$size" \
    --virtual-time-budget="$budget" "$@"
}

PASSED=true
# check <window size> <scenario>
check() {
  RESULT=$(browse "$1" "$CHECK_BUDGET_MS" --dump-dom "http://127.0.0.1:$PORT/ui-tests/harness.html?scenario=$2" 2>/dev/null \
    | sed -n 's:.*<output id="result" hidden="">\(.*\)</output>.*:\1:p')
  echo "$2: ${RESULT:-no result}"
  if [ "$RESULT" != "PASS" ]; then
    PASSED=false
  fi
}

# screenshots <window size> <file suffix> <scenario>
screenshots() {
  url="http://127.0.0.1:$PORT/ui-tests/harness.html?scenario=$3"
  browse "$1" "$SCREENSHOT_BUDGET_MS" --screenshot="$OUT/$3$2-light.png" "$url" 2>/dev/null
  browse "$1" "$SCREENSHOT_BUDGET_MS" --force-dark-mode --blink-settings=preferredColorScheme=0 \
    --screenshot="$OUT/$3$2-dark.png" "$url" 2>/dev/null
}

for checks in $CHECKS; do
  check "$SIZE" "$checks"
done
for checks in $NARROW_CHECKS; do
  check "$NARROW_SIZE" "$checks"
done

for scenario in $SCENARIOS; do
  screenshots "$SIZE" "" "$scenario"
done
for scenario in $NARROW_SCENARIOS; do
  screenshots "$NARROW_SIZE" "-narrow" "$scenario"
done
echo "screenshots: $OUT"
[ "$PASSED" = true ]
