#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

if [[ ${1-} != --inside ]]; then
    output=${1:-$(mktemp -d /tmp/bluebubbles-wayland-seam.XXXXXX)}
    mkdir -p "$output"
    output=$(realpath "$output")
    cargo build --locked --quiet
    printf 'Wayland captures: %s\n' "$output"
    for scale in 1.25 1.5; do
        mkdir -p "$output/$scale"
        if ! env -u WAYLAND_DISPLAY -u DISPLAY -u SWAYSOCK -u HYPRLAND_INSTANCE_SIGNATURE \
            XDG_CURRENT_DESKTOP=sway dbus-run-session -- \
            bash "$0" --inside "$output/$scale" "$scale" >"$output/$scale/session.log" 2>&1; then
            tail -25 "$output/$scale/session.log" >&2
            exit 1
        fi
        grep 'Header seam passed' "$output/$scale/session.log"
    done
    exit 0
fi

output=$2
scale=$3
export XDG_RUNTIME_DIR="$(mktemp -d "$output/runtime.XXXXXX")"
export XDG_CONFIG_HOME="$output/config" XDG_CACHE_HOME="$output/cache"
export WLR_BACKENDS=headless WLR_HEADLESS_OUTPUTS=1 WLR_LIBINPUT_NO_DEVICES=1 WLR_RENDERER=pixman
export GDK_BACKEND=wayland GTK_A11Y=none ADW_DEBUG_COLOR_SCHEME=prefer-dark
unset GDK_SCALE GDK_DPI_SCALE
cat >"$output/sway.conf" <<CONF
output HEADLESS-1 mode 1600x900 scale $scale
default_border none
default_floating_border none
for_window [app_id="org.gtk.Demo4"] floating enable, resize set 240 140, move position 800 400
CONF

sway -c "$output/sway.conf" >"$output/sway.log" 2>&1 &
sway_pid=$!
app=
demo=
trap 'kill "$sway_pid" ${app:+"$app"} ${demo:+"$demo"} 2>/dev/null || true' EXIT
for _ in $(seq 1 100); do
    ipc=$(find "$XDG_RUNTIME_DIR" -maxdepth 1 -type s -name 'sway-ipc*.sock' -print -quit)
    socket=$(find "$XDG_RUNTIME_DIR" -maxdepth 1 -type s -name 'wayland-*' -print -quit)
    [[ -n "$ipc" && -n "$socket" ]] && break
    sleep 0.05
done
[[ -n "$ipc" && -n "$socket" ]]
export SWAYSOCK="$ipc" WAYLAND_DISPLAY="${socket##*/}"

wait_focused() {
    for _ in $(seq 1 100); do
        if swaymsg -rt get_tree | jq -e --arg id "$1" '.. | objects | select(.app_id == $id and .focused)' >/dev/null; then
            return 0
        fi
        sleep 0.05
    done
    printf 'Window did not receive focus: %s\n' "$1" >&2
    return 1
}

./target/debug/bluebubbles-native --patterns >"$output/app.log" 2>&1 &
app=$!
wait_focused co.lxe.BlueBubblesNative.Patterns
sleep 0.25
grim -o HEADLESS-1 "$output/active.png"

gtk4-demo >"$output/demo.log" 2>&1 &
demo=$!
wait_focused org.gtk.Demo4
sleep 0.3
grim -o HEADLESS-1 "$output/backdrop.png"

python3 - "$output" "$scale" <<'PY'
from pathlib import Path
import sys

from PIL import Image

output = Path(sys.argv[1])
scale = float(sys.argv[2])
x = round(316 * scale)
y = round(42 * scale)
states = {}
for state in ("active", "backdrop"):
    with Image.open(output / (state + ".png")) as file:
        frame = file.convert("RGB")
    color = frame.getpixel((x - 5, y))
    # The separator stroke itself can cover two physical pixels at fractional scaling.
    assert all(frame.getpixel((i, y)) == color for i in range(x + 3, x + 14)), (state, x, y)
    assert frame.getpixel((x, y)) != color, (state, x, y)
    states[state] = color

assert states["active"] != states["backdrop"], states
print(f"Header seam passed at {scale:g}× for focused and unfocused windows.")
PY
