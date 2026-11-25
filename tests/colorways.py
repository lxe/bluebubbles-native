"""Check painted GTK colorways and text size, including restart persistence.

Run with `xvfb-run -a dbus-run-session -- python3 tests/colorways.py`.
"""

import argparse
from contextlib import ExitStack
import json
import os
from pathlib import Path
import subprocess
import tempfile

from thread_switch import close_display, image, open_display, poll, stop, xdotool


# Each tuple contains the setting key, canvas, sidebar, outgoing bubble, and accent.
COLORWAYS = (
    ("system", "121b24", "192633", "294a64", "91cbff"),
    ("graphite", "0d0e11", "14161a", "24374d", "a3c9f5"),
    ("champagne", "100e0d", "181513", "382c22", "dbbf9c"),
    ("periwinkle", "0d0e17", "151621", "2d3251", "b2bcfa"),
    ("neon", "08090b", "101115", "30234b", "5aebe2"),
    ("porcelain", "f5f7fb", "eaf0f8", "dee8fb", "264dad"),
    ("ivory", "f7f4ed", "eee8dd", "f0dfd1", "8a4930"),
)
ROW_CENTERS = range(266, 555, 48)


def exercise(binary, output):
    env = dict(os.environ, GDK_BACKEND="x11", GSK_RENDERER="cairo", GTK_A11Y="none",
               G_DEBUG="fatal-criticals", GDK_SCALE="1", GTK_USE_PORTAL="0",
               ADW_DEBUG_COLOR_SCHEME="prefer-dark", ADW_DEBUG_HIGH_CONTRAST="0",
               XDG_CONFIG_HOME=str(output / "config"), XDG_CACHE_HOME=str(output / "cache"))
    config = output / "config/bluebubbles-native/preferences.json"
    config.parent.mkdir(parents=True)
    config.write_text(json.dumps({"sidebar_width": 316, "notifications": False, "colorway": "unknown"}))

    with ExitStack() as stack:
        def launch(command, log_name, environment=None):
            log = stack.enter_context((output / log_name).open("wb"))
            process = subprocess.Popen(command, env=environment, stdout=log, stderr=log)
            stack.callback(stop, process)
            return process

        display = open_display(None)
        assert display, "Run the test inside Xvfb"
        stack.callback(close_display, display)
        launch(["openbox"], "openbox.log")

        def open_app(log_name, **environment):
            app = launch([str(binary), "--patterns"], log_name, env | environment)
            win = int(xdotool("search", "--sync", "--onlyvisible", "--name",
                              "^BlueBubbles Patterns$").splitlines()[0])
            xdotool("windowactivate", "--sync", win)
            return app, win

        app, win = open_app("app.log")

        def click(x, y):
            xdotool("mousemove", "--window", win, x, y, "click", 1)

        def pixel(x, y):
            return bytes(image(display, win, x, y, 1, 1).getpixel((0, 0))).hex()

        def capture(name):
            image(display, win, 0, 0, 1024, 700).save(output / f"{name}.png")

        def assert_painted(theme):
            key, canvas, sidebar, outgoing, _ = theme
            poll(lambda: pixel(950, 440) == canvas and pixel(10, 500) == sidebar
                 and pixel(730, 118) == outgoing)
            capture(key)

        def assert_checked(index, accent):
            try:
                poll(lambda: [i for i, y in enumerate(ROW_CENTERS) if pixel(826, y) == accent]
                     == [index])
            except AssertionError:
                capture(f"picker-failure-{index}")
                raise

        def open_picker(index, accent):
            xdotool("key", "ctrl+comma")
            poll(lambda: pixel(812, 432) == accent)
            click(220, 232)
            assert_checked(index, accent)

        def wait_size(size):
            poll(lambda: config.exists() and json.loads(config.read_text()).get("text_size") == size)

        def message_ink():
            first_bubble = image(display, win, 345, 73, 405, 36)
            return sum(count for count, rgb in first_bubble.getcolors(14600) if min(rgb) > 160)

        click(120, 85)
        assert_painted(COLORWAYS[0])
        normal_ink = message_ink()
        click(435, 660)
        xdotool("type", "Draft preserved")
        draft = image(display, win, 389, 653, 74, 14)
        for key, size in (("ctrl+plus", 110), ("ctrl+equal", 120), ("ctrl+KP_Add", 130),
                          ("ctrl+minus", 120), ("ctrl+KP_Subtract", 110)):
            xdotool("key", key)
            wait_size(size)
        poll(lambda: message_ink() > normal_ink)
        xdotool("key", "ctrl+0")
        wait_size(100)
        poll(lambda: message_ink() == normal_ink)
        poll(lambda: draft.tobytes() == image(display, win, 389, 653, 74, 14).tobytes())
        open_picker(0, COLORWAYS[0][4])

        def minus_glyph_bounds():
            crop = image(display, win, 720, 152, 16, 14)
            pts = [(x, y) for y in range(14) for x in range(16) if max(crop.getpixel((x, y))) > 150]
            assert pts, "Minus icon missing"
            return (min(x for x, _ in pts), min(y for _, y in pts),
                    max(x for x, _ in pts), max(y for _, y in pts))

        normal_minus = minus_glyph_bounds()
        assert normal_minus[2] - normal_minus[0] >= 7 and normal_minus[3] - normal_minus[1] <= 2, normal_minus
        click(828, 159)
        wait_size(110)
        click(728, 159)
        wait_size(100)
        xdotool("key", "--repeat", 8, "--delay", 45, "ctrl+equal")
        wait_size(160)
        capture("appearance-large")
        # Minus button must not drift horizontally or vertically when text size scales to 160%.
        assert minus_glyph_bounds() == normal_minus, "Text size control drifted at 160%"
        click(778, 159)
        wait_size(100)
        capture("appearance")
        for index, theme in enumerate(COLORWAYS[1:], start=1):
            click(520, ROW_CENTERS[index])
            assert_checked(index, theme[4])
            saved = json.loads(config.read_text())
            assert saved["colorway"] == theme[0]
            assert saved["notifications"] is False
            xdotool("key", "Escape")
            assert_painted(theme)
            if index < len(COLORWAYS) - 1:
                open_picker(index, theme[4])

        # An explicit light theme stays selected after restart under a dark desktop.
        stop(app)
        app, win = open_app("restart.log")
        click(120, 85)
        assert_painted(COLORWAYS[-1])
        open_picker(len(COLORWAYS) - 1, COLORWAYS[-1][4])
        click(520, ROW_CENTERS[0])
        assert_checked(0, COLORWAYS[0][4])
        assert json.loads(config.read_text())["colorway"] == "system"
        xdotool("key", "Escape")
        assert_painted(COLORWAYS[0])

        xdotool("key", "--repeat", 6, "--delay", 45, "ctrl+minus")
        wait_size(80)
        poll(lambda: message_ink() < normal_ink)
        stop(app)
        app, win = open_app("text-restart.log")
        click(120, 85)
        poll(lambda: 0 < message_ink() < normal_ink)
        xdotool("key", "ctrl+0")
        wait_size(100)
        poll(lambda: message_ink() == normal_ink)

        # Follow System also picks up a light desktop on launch.
        stop(app)
        app, win = open_app("system-light.log", ADW_DEBUG_COLOR_SCHEME="prefer-light")
        click(120, 85)
        poll(lambda: pixel(950, 440) == "f7f9fc")


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=Path("target/debug/bluebubbles-native"))
    args = parser.parse_args()
    output = Path(tempfile.mkdtemp(prefix="bluebubbles-native-colorways."))
    print(f"Screenshots and logs in {output}", flush=True)
    exercise(args.binary.resolve(), output)
    print("Colorways and text size painted and persisted across restart")
