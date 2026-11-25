#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

if [[ ${1-} != --display ]]; then
    output=$(mktemp -d /tmp/bluebubbles-thread-features.XXXXXX)
    printf 'Thread captures: %s\n' "$output"
    cargo build --locked --quiet
    if ! env -u WAYLAND_DISPLAY -u HYPRLAND_INSTANCE_SIGNATURE XDG_CURRENT_DESKTOP=GNOME \
        xvfb-run -a -s '-screen 0 1280x900x24' dbus-run-session -- bash "$0" --display "$output" >"$output/session.log" 2>&1; then
        tail -20 "$output/session.log" >&2
        exit 1
    fi
    grep 'Group thread checks passed' "$output/session.log"
    exit
fi

output=$2
export GDK_BACKEND=x11 GSK_RENDERER=cairo GTK_A11Y=none ADW_DEBUG_COLOR_SCHEME=prefer-dark
export XDG_CONFIG_HOME="$output/config" XDG_CACHE_HOME="$output/cache"
export RUST_BACKTRACE=1 G_DEBUG=fatal-criticals
export BLUEBUBBLES_NATIVE_PROFILE=1
openbox >"$output/openbox.log" 2>&1 &
wm=$!
python3 tests/fixture.py --ready "$output/port" --receipts "$output/receipts.jsonl" --group-chat \
    --reaction-delay 1.3 --reject-reaction dislike \
    >"$output/fixture.log" 2>&1 &
fixture=$!
app=
trap 'kill "$wm" "$fixture" ${app:+"$app"} 2>/dev/null || true' EXIT
for _ in $(seq 1 80); do
    [[ -s "$output/port" ]] && break
    sleep .1
done
export BLUEBUBBLES_NATIVE_SERVER="http://127.0.0.1:$(cat "$output/port")/"
export BLUEBUBBLES_NATIVE_PASSWORD=fixture-secret
python3 - <<'PY'
import json, os
from pathlib import Path
from uuid import NAMESPACE_URL, uuid5

path = (Path(os.environ["XDG_CACHE_HOME"]) / "bluebubbles-native" / "v1"
        / str(uuid5(NAMESPACE_URL, os.environ["BLUEBUBBLES_NATIVE_SERVER"])) / "links")
path.mkdir(parents=True)
url = "https://example.invalid/group-preview"
(path / f"{uuid5(NAMESPACE_URL, url)}.json").write_text(json.dumps({
    "title": "Group trail workshop",
    "description": "This is a deliberately long link preview that describes the route to the trailhead. "
                   "Bring water and an extra layer because the wind picks up in the afternoon. "
                   "We will stop at the ridge, then head back to the parking lot.",
}))
PY

./target/debug/bluebubbles-native >"$output/app.log" 2>&1 &
app=$!
win=$(timeout 8s xdotool search --sync --onlyvisible --name '^BlueBubbles Native$' | head -1)
xdotool windowactivate --sync "$win"
click() {
    xdotool mousemove --window "$win" "$1" "$2" click --delay 0 "${3:-1}"
}
snap() {
    import -window "$win" "$output/$1.png"
}
sleep .55
click 130 85
sleep .65
snap group
if [[ ${BLUEBUBBLES_CAPTURE_GROUP_ONLY-} == 1 ]]; then
    printf 'Group thread checks passed\n'
    exit
fi

message_point() {
    tesseract "$output/$1.png" stdout --psm 11 tsv 2>/dev/null |
        awk -v word="$2" -v edge="$3" -v region="${4-}" '
            $12 == word && $7 >= edge && (region == "" || $7 <= region) && !found {
                print $7 + int($9 / 2), $8 + int($10 / 2)
                found = 1
            }'
}
read -r target_x target_y < <(message_point group Meet 330)
read -r link_x link_y < <(message_point group example.invalid 350 600)
out_y=$(python3 - "$output/group.png" <<'PY2'
from PIL import Image
import sys
image = Image.open(sys.argv[1]).convert("RGB")
print(next(y for y in range(400, 625) if image.getpixel((802, y)) == (41, 74, 100)) + 17)
PY2
)
incoming_badge_x=$((target_x + 129))
incoming_badge_y=$((target_y - 21))
outgoing_badge_x=619
outgoing_badge_y=$((out_y - 20))
xdotool mousemove --window "$win" "$incoming_badge_x" "$incoming_badge_y"
sleep .32
snap reactions-hover
click "$incoming_badge_x" "$incoming_badge_y"
xdotool mousemove --window "$win" 550 665
sleep .32
snap reactions-pinned
xdotool key Escape
sleep .23
snap reactions-closed
xdotool mousemove --window "$win" "$outgoing_badge_x" "$outgoing_badge_y"
sleep .32
snap reactions-outgoing
xdotool mousemove --window "$win" 550 665
sleep .2

click "$target_x" "$target_y" 3
sleep .25
import -window root "$output/message-actions.png"
xdotool key Home Return
sleep .22
date +%s%3N >"$output/reaction-paint-time"
snap reacted
click "$target_x" "$target_y" 3
sleep .25
xdotool key Home Return
sleep .22
snap reaction-removed
sleep 2.5
snap reaction-confirmed

click "$target_x" "$target_y" 3
sleep .25
xdotool key Home Right Right Return
sleep .22
snap reaction-rejected-pending
sleep 1.35
snap reaction-rejected

click "$link_x" "$link_y" 3
sleep .25
import -window root "$output/link-actions.png"
xdotool key End Up Return
timeout 2s xclip -selection clipboard -o >"$output/copied-url.txt"
click "$link_x" "$link_y" 3
sleep .25
xdotool key End Return
sleep .35
snap reply-draft
xdotool type --delay 8 'I will bring a map.'
xdotool key Return
sleep .65
snap reply-sent

xdotool windowsize --sync "$win" 450 680
sleep .45
snap group-narrow

python3 - "$output" "$target_y" "$out_y" <<'PY'
import json
from pathlib import Path
from PIL import Image
import subprocess
import sys

root = Path(sys.argv[1])
target_y, outgoing_y = map(int, sys.argv[2:])
events = [json.loads(line) for line in (root / "receipts.jsonl").read_text().splitlines()]
react = [event for event in events if event["event"] == "reaction"]
assert [(event["target"], event["reaction"]) for event in react] == [
    ("group-parent", "love"), ("group-parent", "-love"), ("group-parent", "dislike"),
], react
assert react[0]["at"] <= int((root / "reaction-paint-time").read_text()) < next(
    event["at"] for event in events if event["event"] == "reaction-reply"
), events
sent = next(event for event in events if event["event"] == "text")
assert not any(event["event"] == "read"
               for event in events[events.index(react[0]):events.index(sent)]), events
assert (sent["method"], sent["selected"], sent["part"], sent["text"]) == (
    "private-api", "group-link-in", 0, "I will bring a map.",
), sent
assert (root / "copied-url.txt").read_text() == "https://example.invalid/group-preview"
assert sum(event["event"] == "group-icon" for event in events) == 1, events

def ocr(name, mode="11", region=None):
    path = root / f"{name}.png"
    if region is not None:
        crop = Image.open(path).crop(region)
        path = root / f"{name}-crop.png"
        crop.resize((crop.width * 2, crop.height * 2)).save(path)
    return subprocess.check_output(["tesseract", str(path), "stdout", "--psm", mode],
                                   text=True, stderr=subprocess.DEVNULL)

group = ocr("group")
assert "Loved the message" not in group, group
timeline_text = ocr("group", region=(330, 65, 1015, 620)).casefold()
for text in ("maya chen", "jo reyes", "meet by the trailhead", "can meet you there",
             "water is packed", "group trail workshop"):
    assert text in timeline_text, (text, timeline_text)
assert any(marker in timeline_text for marker in ("hours ago", "yesterday", "just now", "nov 24")), timeline_text
assert timeline_text.count("jo reyes") == 1, timeline_text
assert timeline_text.count("can meet you there.") == 1, timeline_text
draft = ocr("reply-draft", "6", (339, 578, 1005, 633))
assert "Replying to Maya Chen" in draft, draft
assert "https://example.invalid/group-preview" in draft, draft
sent_text = ocr("reply-sent", "6", (690, 520, 1006, 618))
assert "will bring a map" in sent_text, sent_text

def widest_span(image, color, region):
    left, top, right, bottom = region
    spans = []
    for y in range(top, bottom):
        points = [x for x in range(left, right) if image.getpixel((x, y)) == color]
        if points:
            spans.append((min(points), max(points)))
    return max(spans, key=lambda span: span[1] - span[0])

wide = Image.open(root / "group.png").convert("RGB")
def reaction_area(name, region):
    return Image.open(root / f"{name}.png").convert("RGB").crop(region)

def gold(pixel):
    r, g, b = pixel
    return r > 200 and g > 130 and b < 95

def gold_count(area):
    return sum(n for n, color in area.getcolors(5000) if gold(color))

def red_count(area):
    return sum(n for n, (r, g, b) in area.getcolors(5000) if r > 160 and r > 2*g and r > 1.5*b)

incoming = (432, target_y - 34, 534, target_y - 10)
outgoing = (583, outgoing_y - 34, 675, outgoing_y - 5)
idle = reaction_area("group", incoming)
hover = reaction_area("reactions-hover", incoming)
def gold_columns(area):
    return [x for x in range(area.width) if any(gold(area.getpixel((x, y))) for y in range(area.height))]

assert min(gold_columns(hover)) <= min(gold_columns(idle)) - 11, "Incoming reactions should spread left"
expanded = Image.open(root / "reactions-hover.png").convert("RGB")
shadow = expanded.crop((524, target_y - 22, 527, target_y - 14))
assert any(r < 18 and g < 27 and b < 36 for _, (r, g, b) in shadow.getcolors(24)), (
    "The offset emoji shadow should darken the background"
)
assert hover.tobytes() == reaction_area("reactions-pinned", incoming).tobytes()
assert idle.tobytes() == reaction_area("reactions-closed", incoming).tobytes()
out_idle = reaction_area("group", outgoing)
out_hover = reaction_area("reactions-outgoing", outgoing)
assert max(gold_columns(out_hover)) >= max(gold_columns(out_idle)) + 11, "Outgoing reactions should spread right"

assert red_count(reaction_area("reacted", incoming)) > 100, "The reaction did not paint provisionally"
for capture in ("reaction-removed", "reaction-confirmed", "reaction-rejected"):
    assert reaction_area(capture, incoming).tobytes() == idle.tobytes(), capture
assert gold_count(reaction_area("reaction-rejected-pending", incoming)) > gold_count(idle) + 100

for x, y in ((37, 91), (580, 30)):
    red, green, blue = wide.getpixel((x, y))
    assert red > 180 and green < 75 and blue > 110, (x, y, red, green, blue)
left = widest_span(wide, (34, 52, 67), (325, 65, 1018, 620))
right = widest_span(wide, (41, 74, 100), (325, 65, 1018, 620))
lane = right[1] - left[0] + 1
assert left[1] - left[0] + 1 <= lane * .75 + 1, left
assert right[1] - right[0] + 1 <= lane * .75 + 1, right
assert left[0] <= 340 and right[1] >= 1000, (left, right)
def light_text(rgb):
    return min(rgb) > 185 and max(rgb) - min(rgb) < 35

bubble_rows = [y for y in range(target_y - 21, target_y + 21) if any(wide.getpixel((x, y)) == (34, 52, 67) for x in range(352, 506))]
glyph_rows = [y for y in range(target_y - 8, target_y + 9) if any(light_text(wide.getpixel((x, y))) for x in range(352, 506))]
assert (glyph_rows[0] - bubble_rows[0], bubble_rows[-1] - glyph_rows[-1]) == (12, 9)

narrow = Image.open(root / "group-narrow.png").convert("RGB")
narrow_span = widest_span(narrow, (41, 74, 100), (20, 70, 430, 605))
assert 275 <= narrow_span[1] - narrow_span[0] + 1 <= 308, narrow_span
print("Group thread checks passed")
PY
