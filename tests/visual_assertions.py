"""Check GUI geometry and behavior against the synthetic desktop captures."""

import csv
import io
import json
from pathlib import Path
import subprocess
import sys

from PIL import Image, ImageOps


if sys.argv[1] == "--has-media":
    image = Image.open(sys.argv[2]).convert("RGB")
    crop = image.crop((230, 170, 790, 550))
    pixel_data = crop.get_flattened_data() if hasattr(crop, "get_flattened_data") else crop.getdata()
    colored = sum(max(rgb) - min(rgb) >= 65 for rgb in pixel_data)
    sys.exit(0 if colored > 8000 else 1)

captures = Path(sys.argv[1])
ACCENT = (145, 203, 255)
SELECTED = (48, 78, 105)
SURFACE = (32, 47, 62)
DIVIDER = (41, 59, 76)
HEADER = (27, 40, 54)
CONTROL = (48, 60, 74)
SIDEBAR = (25, 38, 51)
CANVAS = (18, 27, 36)


def screen(name):
    return Image.open(captures / f"{name}.png").convert("RGB")


def ocr(name):
    result = subprocess.check_output(
        ["tesseract", str(captures / f"{name}.png"), "stdout", "--psm", "11", "tsv"],
        stderr=subprocess.DEVNULL,
        text=True,
    )
    return [item for item in csv.DictReader(io.StringIO(result), delimiter="\t") if item["text"]]


def word(rows, text):
    return next(row for row in rows if row["text"].casefold() == text.casefold())


def sentence(rows, text):
    combined = " ".join(row["text"] for row in rows)
    assert text.casefold() in combined.casefold(), f"Missing {text!r} in {combined!r}"


def pixels(image):
    if hasattr(image, "get_flattened_data"):
        return image.get_flattened_data()
    return image.getdata()


def bounds(image, region, color):
    xx, yy, right, bottom = region
    points = [(x, y) for y in range(yy, bottom) for x in range(xx, right)
              if image.getpixel((x, y)) == color]
    assert points, f"Missing fill {color} in {region}"
    return min(x for x, _ in points), min(y for _, y in points), max(x for x, _ in points), max(y for _, y in points)


def toolbar_bounds(image, right_edge, narrow=False):
    left_edge = 5
    controls = []
    for x in range(left_edge, right_edge):
        if sum(image.getpixel((x, y)) in (CONTROL, SURFACE) for y in range(8, 52)) <= 5:
            continue
        if controls and x == controls[-1][1] + 1:
            controls[-1][1] = x
        else:
            controls.append([x, x])
    assert len(controls) == (4 if narrow else 3), controls
    frames = [bounds(image, (left, 8, right + 1, 52), SURFACE if index == 1 else CONTROL)
              for index, (left, right) in enumerate(controls)]
    assert all(top == 13 and bottom == 46 for _, top, _, bottom in frames), frames
    assert all(right - left + 1 == 34 for index, (left, _, right, _) in enumerate(frames) if index != 1), frames
    assert frames[0][0] - left_edge == right_edge - frames[-1][2] - 1 == 8, frames
    assert all(right[0] - left[2] - 1 == 8 for left, right in zip(frames, frames[1:])), frames
    assert all(image.getpixel((x, 55)) == DIVIDER for x in (left_edge + 2, right_edge - 3))
    return frames


def assert_syncing(image, frame):
    x, y, right, bottom = frame
    icon_area = image.crop((x + 7, y + 7, right - 6, bottom - 6))
    assert sum(blue >= 190 and blue - red >= 72 for red, _, blue in pixels(icon_area)) >= 3, frame


preferences = ocr("preferences")
assert abs(int(word(preferences, "Status")["left"]) - int(word(preferences, "Remember")["left"])) <= 3
# Preferences sidebar title is centered in the 200px sidebar (x = 142..342 -> 242).
assert abs(int(word(preferences, "Preferences")["left"]) + int(word(preferences, "Preferences")["width"]) / 2 - 242) <= 6
error = ImageOps.invert(screen("error").crop((360, 340, 860, 405)).convert("L"))
error = error.resize((1000, 130))
error.save(captures / "error-status.png")
result = subprocess.check_output(
    ["tesseract", str(captures / "error-status.png"), "stdout", "--psm", "6"],
    text=True, stderr=subprocess.DEVNULL)
assert "Connection problem" in result, result

hover = screen("hover")
thread = screen("thread")
assert SELECTED not in pixels(hover.crop((175, 60, 300, 110)))
selection = bounds(thread, (0, 54, 320, 116), SELECTED)
search = bounds(thread, (48, 5, 265, 48), SURFACE)
assert abs(search[1] - 13) <= 3 and search[3] - search[1] >= 25, search
wide = toolbar_bounds(thread, 321)
syncing = screen("toolbar-syncing")
assert toolbar_bounds(syncing, 321) == wide
assert_syncing(syncing, wide[2])
assert (selection[0], selection[1], selection[2]) == (wide[0][0], 64, wide[-1][2]), selection
sentence(ocr("thread"), "Check the trail markers.")
assert all(row["text"] != "continues" or int(row["left"]) > 320 for row in ocr("thread")), "Sidebar preview wrapped"

incoming = bounds(thread, (330, 53, 1018, 350), (34, 52, 67))
composer = bounds(thread, (330, 625, 1018, 685), SURFACE)
outgoing = bounds(screen("sent"), (690, 440, 1018, 635), (41, 74, 100))
assert abs(incoming[0] - composer[0]) <= 2, (incoming, composer)
assert abs(outgoing[2] - composer[2]) <= 2, (outgoing, composer)

one = screen("composer-one")
many = screen("composer-many")
# Measure the composer away from the floating jump button, which shares its surface color.
single = bounds(one, (335, 525, 945, 688), SURFACE)
expanded = bounds(many, (335, 525, 945, 688), SURFACE)
assert single[3] - single[1] < 48 and expanded[3] - expanded[1] >= 70, (single, expanded)
for name in ("composer-one", "composer-many"):
    image = screen(name)
    # Keep the icon small and exclude the composer's focus border.
    release = image.crop((940, 642, 991, 670))
    accent = [color for color in pixels(release)
              if max(abs(actual - expected) for actual, expected in zip(color, ACCENT)) <= 31]
    assert 5 <= len(accent) < 150, f"Send icon painted a large solid control in {name}: {len(accent)}"
multiline = ocr("composer-many")
for index, label in enumerate(("Second", "Third", "Fourth")):
    if index:
        assert int(word(multiline, label)["top"]) > int(word(multiline, ("Second", "Third")[index - 1])["top"])

def menu_words(name, region):
    target = captures / f"{name}-crop.png"
    crop = screen(name).crop(region)
    crop.resize((crop.width * 3, crop.height * 3)).save(target)
    result = subprocess.check_output(
        ["tesseract", str(target), "stdout", "--psm", "11"],
        stderr=subprocess.DEVNULL, text=True)
    return " ".join(result.split())

for name in ("image-immediate", "image-viewer", "video-viewer"):
    assert screen(name).size == thread.size, "Media opened outside the app window"
    words = " ".join(item["text"] for item in ocr(name))
    assert "Loading media" not in words and "Trail markers.png" not in words and "Trail video.webm" not in words, words

copied = screen("copied")
assert copied.size == (320, 200), copied.size
assert copied.getpixel((0, 0)) == (22, 60, 133)
assert copied.getpixel((319, 199)) == (101, 159, 212)

# Compare the painted glyph and ring centers on the rendered lightbox.
viewer = screen("image-viewer")
ring = bounds(viewer, (895, 45, 950, 97), ACCENT)
glyph = bounds(viewer, (901, 51, 944, 94), (246, 249, 252))
assert ring[2] - ring[0] == ring[3] - ring[1], ring
assert (ring[0] + ring[2], ring[1] + ring[3]) == (glyph[0] + glyph[2], glyph[1] + glyph[3]), (ring, glyph)

for line in ("Open in Viewer", "Copy", "Download"):
    assert line in menu_words("media-menu", (480, 480, 680, 650)), line
for line in ("Copy", "Download"):
    assert line in menu_words("viewer-menu", (485, 480, 670, 630)), line
for line in ("Pin Conversation", "Archive Conversation"):
    assert line in menu_words("chat-menu", (165, 245, 415, 390)), line

sentence(ocr("sent"), "Fourth line")
assert all(item["text"] != "Sent" for item in ocr("sent"))
preview = menu_words("link-preview", (840, 62, 1010, 170))
assert "example.invalid" in preview and "pluginPayloadAttachment" not in preview and "/article" not in preview, preview

pinned_sidebar = [row for row in ocr("pinned") if int(row["left"]) < 320]
assert int(word(pinned_sidebar, "Bike")["top"]) < int(word(pinned_sidebar, "Maya")["top"])
assert all(item["text"] != "Bike" for item in ocr("archived-sidebar"))
sentence(ocr("archive"), "Bike Workshop")
assert "No archived chats" in menu_words("restored", (350, 260, 860, 440))
sentence(ocr("narrow-thread"), "Maya Chen")
sentence(ocr("narrow-reopened"), "Maya Chen")
assert SELECTED not in pixels(screen("narrow").crop((10, 55, 420, 250)))
narrow = toolbar_bounds(screen("narrow"), 445, narrow=True)
narrow_syncing = screen("narrow-syncing")
assert toolbar_bounds(narrow_syncing, 445, narrow=True) == narrow
assert_syncing(narrow_syncing, narrow[2])

# The paned handle paints one line. Its remaining interactive pixels match the thread.
dividers = {}
for name in ("thread", "resized"):
    image = screen(name)
    divider = [x for x in range(220, 520) if image.getpixel((x, 350)) == DIVIDER]
    assert len(divider) == 1, (name, divider)
    x = divider[0]
    dividers[name] = x
    assert all(image.getpixel((i, 350)) == CANVAS for i in range(x + 1, x + 8)), name
    assert all(image.getpixel((i, 49)) == HEADER
               for i in range(x - 8, x + 10) if i != x), name
    rows = [y for y in range(42, 70) if all(image.getpixel((i, y)) == DIVIDER for i in range(x - 8, x + 8))]
    assert len(rows) == 1, (name, rows)
assert dividers["resized"] > dividers["thread"] + 30, dividers
scroll = screen("sidebar-track")
x = dividers["thread"]
assert scroll.getpixel((x, 180)) == DIVIDER
assert scroll.getpixel((x - 1, 180)) != SIDEBAR
assert scroll.getpixel((x + 1, 180)) == CANVAS

sentence(ocr("offline"), "Maya Chen")
sentence(ocr("offline-thread"), "Here is the field log")

events = [json.loads(line) for line in (captures / "receipts.jsonl").read_text().splitlines()]
queries = [event["limit"] for event in events if event["event"] == "chats"]
assert queries.count(500) == 2 and 100 in queries, queries
assert any(event["event"] == "text" and "\nFourth line" in event["text"] for event in events)
assert {"fixture-image", "fixture-video"} <= {event["guid"] for event in events if event["event"] == "download"}
print("Visual and transport checks passed.")
