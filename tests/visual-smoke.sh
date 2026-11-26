#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

if [[ ${1-} != --inside ]]; then
    output=${1:-$(mktemp -d /tmp/bluebubbles-native-visual.XXXXXX)}
    mkdir -p "$output"
    output=$(realpath "$output")
    cargo build --locked --quiet
    printf 'Screenshots and fixture receipts: %s\n' "$output"
    if ! env -u WAYLAND_DISPLAY -u HYPRLAND_INSTANCE_SIGNATURE XDG_CURRENT_DESKTOP=GNOME xvfb-run -a -s '-screen 0 1280x900x24' dbus-run-session \
        -- bash "$0" --inside "$output" >"$output/session.log" 2>&1; then
        grep -E 'Traceback|AssertionError|Expected text|Media did not appear' "$output/session.log" >&2 || tail -12 "$output/session.log" >&2
        exit 1
    fi
    grep 'Visual and transport checks passed' "$output/session.log"
    exit 0
fi

output=$2
export GDK_BACKEND=x11 GSK_RENDERER=cairo GTK_A11Y=none
export XDG_CONFIG_HOME="$output/config" XDG_CACHE_HOME="$output/cache"
export ADW_DEBUG_COLOR_SCHEME=prefer-dark GDK_SCALE=1 GTK_USE_PORTAL=0 RUST_BACKTRACE=1 G_DEBUG=fatal-criticals
rm -f "$output/port" "$output/receipts.jsonl"

openbox >"$output/openbox.log" 2>&1 &
wm=$!
ffmpeg -nostdin -loglevel error -y -f lavfi -i 'testsrc2=size=320x200:rate=12' \
    -t 1.6 -c:v libvpx-vp9 "$output/example.webm"
python3 tests/fixture.py --ready "$output/port" --receipts "$output/receipts.jsonl" \
    --video "$output/example.webm" --extra-chats 30 --original-delay 1.1 \
    --query-delay 1 >"$output/fixture.log" 2>&1 &
fixture=$!
app=
trap 'kill "$wm" "$fixture" ${app:+"$app"} 2>/dev/null || true' EXIT

for _ in $(seq 1 80); do
    [[ -s "$output/port" ]] && break
    sleep 0.1
done
port=$(cat "$output/port")
url="http://127.0.0.1:$port/"
name="BlueBubbles Native"

open_window() {
    ./target/debug/bluebubbles-native >"$output/$1.log" 2>&1 &
    app=$!
    window=$(timeout 8s xdotool search --sync --onlyvisible --name "^$name$" | head -1)
    xdotool windowactivate --sync "$window"
    sleep 0.45
}
stop_window() {
    kill "$app"
    wait "$app" 2>/dev/null || true
    app=
}
snap() {
    import -window "$window" "$output/$1.png"
}
wait_viewer() {
    local capture=$1
    local title=$2
    for _ in $(seq 1 12); do
        sleep 0.24
        snap "$capture"
        if tesseract "$output/$capture.png" stdout --psm 11 2>/dev/null | grep -Fiq "$title"; then
            return 0
        fi
    done
    printf 'Expected text did not appear: %s
' "$title" >&2
    return 1
}
wait_media() {
    for _ in $(seq 1 18); do
        sleep 0.12
        snap "$1"
        if python3 tests/visual_assertions.py --has-media "$output/$1.png"; then
            return 0
        fi
    done
    printf 'Media did not appear: %s\n' "$1" >&2
    return 1
}
click() {
    xdotool mousemove --window "$window" "$1" "$2"
    xdotool click "${3:-1}"
}
read_count() {
    if [[ -f "$output/receipts.jsonl" ]]; then
        python3 - "$output/receipts.jsonl" <<'PY'
import json, sys
with open(sys.argv[1]) as handle:
    print(sum(json.loads(line)["event"] == "read" for line in handle))
PY
    else
        printf '0\n'
    fi
}

open_window disconnected
snap disconnected
click 672 414
sleep 0.45
snap preferences
xdotool type --delay 12 "$url"
click 520 256
xdotool type --delay 15 wrong
click 812 432
sleep 1.0
snap error
stop_window

export BLUEBUBBLES_NATIVE_SERVER="$url" BLUEBUBBLES_NATIVE_PASSWORD=fixture-secret
mkdir -p "$XDG_CONFIG_HOME/bluebubbles-native"
printf '{"server":"%s","sidebar_width":316}\n' "$url" >"$XDG_CONFIG_HOME/bluebubbles-native/preferences.json"
open_window connected
snap toolbar-syncing
sleep 0.8
before=$(read_count)
xdotool mousemove --window "$window" 125 83
sleep 0.4
snap hover
[[ $(read_count) == "$before" ]]
click 125 83
sleep 1.0
snap thread
[[ $(read_count) -gt "$before" ]]

click 442 390 3
sleep 0.35
import -window root "$output/media-menu.png"
xdotool key Escape
sleep 0.28
click 442 390
sleep 0.12
snap image-immediate
python3 tests/visual_assertions.py --has-media "$output/image-immediate.png"
wait_media image-viewer
click 442 390 3
sleep 0.35
import -window root "$output/viewer-menu.png"
xdotool key Down Return
for _ in $(seq 1 18); do
    if timeout 1s xclip -selection clipboard -t image/png -o >"$output/copied.png" 2>/dev/null; then
        break
    fi
    sleep .12
done
[[ -s "$output/copied.png" ]]
click 923 70
sleep 0.5
click 442 535
wait_media video-viewer
xdotool key Escape
sleep 0.35

xdotool mousemove --window "$window" 640 270 click --repeat 38 --delay 12 4
sleep 0.28
snap history-boundary
click 669 79
sleep 0.55
snap older
grep -q '"offset": 50' "$output/receipts.jsonl"

click 620 670
xdotool type --delay 4 'A compact message'
sleep 0.2
snap composer-one
for sentence in 'Second line' 'Third line' 'Fourth line'; do
    xdotool key shift+Return
    xdotool type --delay 4 "$sentence"
done
sleep 0.3
snap composer-many
xdotool key Return
sleep 0.65
snap sent
grep -q '"event": "text"' "$output/receipts.jsonl"

click 125 204
sleep .45
snap link-preview
click 125 83
sleep .3
click 125 144 3
sleep .28
import -window root "$output/chat-menu.png"
xdotool key Return
sleep .22
snap neutral-menu
xdotool key Down Return
sleep .35
snap pinned
click 125 83 3
sleep .24
xdotool key Down Down Return
sleep .35
snap archived-sidebar
xdotool mousemove --window "$window" 319 250
sleep .35
snap sidebar-track
xdotool key ctrl+comma
sleep .35
click 220 190
wait_viewer archive 'Bike Workshop'
click 815 160
sleep .35
snap restored
xdotool key Escape
sleep .35

xdotool mousemove --window "$window" 324 392
xdotool mousedown 1
xdotool mousemove --window "$window" 398 392
xdotool mouseup 1
sleep 0.4
snap resized
xdotool windowsize --sync "$window" 450 680
sleep 0.45
snap narrow-thread
click 28 30
xdotool mousemove --window "$window" 120 130
sleep .45
snap narrow
click 336 30
xdotool mousemove --window "$window" 120 130
sleep .2
snap narrow-syncing
click 110 88
sleep .45
snap narrow-reopened
stop_window

./target/debug/bluebubbles-native --check-connection >"$output/warm-check.log"
kill "$fixture"
wait "$fixture" 2>/dev/null || true
open_window offline
sleep 0.65
snap offline
wait_viewer offline-thread 'Maya Chen'
stop_window
python3 tests/visual_assertions.py "$output"
