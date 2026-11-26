"""Time the first painted message bubbles while switching between fixture chats.

Run inside Xvfb with a private D-Bus session, for example:
xvfb-run -a dbus-run-session -- python3 tests/thread_switch.py
"""

import argparse
from contextlib import ExitStack
import ctypes as c
import json
import os
from pathlib import Path
import statistics
import subprocess
import tempfile
import time

from PIL import Image, ImageChops


class XImage(c.Structure):
    _fields_ = [
        ("width", c.c_int), ("height", c.c_int), ("xoffset", c.c_int), ("format", c.c_int),
        ("data", c.c_void_p), ("byte_order", c.c_int), ("bitmap_unit", c.c_int),
        ("bitmap_bit_order", c.c_int), ("bitmap_pad", c.c_int), ("depth", c.c_int),
        ("bytes_per_line", c.c_int), ("bits_per_pixel", c.c_int),
    ]


def signature(function, result, *arguments):
    function.restype = result
    function.argtypes = arguments
    return function


x11 = c.CDLL("libX11.so.6")
open_display = signature(x11.XOpenDisplay, c.c_void_p, c.c_char_p)
get_image = signature(x11.XGetImage, c.POINTER(XImage), c.c_void_p, c.c_ulong,
                      c.c_int, c.c_int, c.c_uint, c.c_uint, c.c_ulong, c.c_int)
destroy_image = signature(x11.XDestroyImage, c.c_int, c.POINTER(XImage))
close_display = signature(x11.XCloseDisplay, c.c_int, c.c_void_p)
BUBBLE = (34, 52, 67)


def image(display, window, left, top, width, height):
    capture = get_image(display, window, left, top, width, height, -1, 2)
    assert capture and capture.contents.bits_per_pixel == 32
    try:
        raw = capture.contents
        return Image.frombytes("RGB", (width, height),
                               c.string_at(raw.data, raw.bytes_per_line * height),
                               "raw", "BGRX", raw.bytes_per_line)
    finally:
        destroy_image(capture)


def poll(condition, timeout=5):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        result = condition()
        if result:
            return result
        time.sleep(.008)
    raise AssertionError("Timed out waiting for the fixture or window")


def stop(process):
    if process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=3)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()


def xdotool(*args):
    return subprocess.check_output(["xdotool", *map(str, args)], text=True, timeout=5).strip()


def exercise(binary, output, baseline, large):
    with ExitStack() as stack:
        def launch(args, log, env=None):
            handle = stack.enter_context((output / log).open("w"))
            process = subprocess.Popen(args, env=env, stdout=handle, stderr=handle)
            stack.callback(stop, process)
            return process

        display = open_display(None)
        assert display, "Run the test in its own Xvfb display"
        stack.callback(close_display, display)
        launch(["openbox"], "wm.log")
        status, receipts = output / "port", output / "requests.jsonl"
        size = ["--extra-chats", "2902", "--query-delay", "0.22"] if large else []
        fixture = launch(["python3", "tests/fixture.py", "--ready", str(status),
                          "--receipts", str(receipts), "--message-delay", "0.8",
                          "--preview-delay", "3", *size], "server.log")
        port = poll(lambda: status.read_text() if status.exists() else "")
        url = f"http://127.0.0.1:{port}/"
        env = dict(os.environ, GDK_BACKEND="x11", GSK_RENDERER="cairo", GTK_A11Y="none",
                   BLUEBUBBLES_NATIVE_PROFILE="1", RUST_BACKTRACE="1",
                   ADW_DEBUG_COLOR_SCHEME="prefer-dark", GDK_SCALE="1",
                   XDG_CONFIG_HOME=str(output / "config"), XDG_CACHE_HOME=str(output / "cache"),
                   BLUEBUBBLES_NATIVE_SERVER=url, BLUEBUBBLES_NATIVE_PASSWORD="fixture-secret")
        config = Path(env["XDG_CONFIG_HOME"]) / "bluebubbles-native" / "preferences.json"
        config.parent.mkdir(parents=True)
        config.write_text(json.dumps({
            "server": url, "sidebar_width": 316,
            "chat_organization": {url: {"archived": {"iMessage;-;+15550200000": "Archived sample"}}}
                if large else {},
        }))

        def open_app(log):
            process = launch([str(binary)], log, env)
            win = int(xdotool("search", "--sync", "--onlyvisible", "--name", "^BlueBubbles Native$").splitlines()[0])
            xdotool("windowactivate", "--sync", win)
            return process, win

        def events(kind):
            return [event for line in receipts.read_text().splitlines()
                    if (event := json.loads(line))["event"] == kind] if receipts.exists() else []

        app, win = open_app("app.log")
        poll(lambda: list((output / "cache").glob("bluebubbles-native/v1/*/conversations.json")))
        time.sleep(.35)

        def painted_many(many):
            # Both chats contain filled bubbles near the top. Only Maya's has more beneath them.
            area = image(display, win, 352, 56, 94, 206)
            upper = area.crop((0, 0, 94, 75)).getcolors(8000)
            lower = area.crop((0, 90, 94, 206)).getcolors(12000)
            return (any(color == BUBBLE and n > 100 for n, color in upper)
                    and any(color == BUBBLE and n > 100 for n, color in lower) == many)

        def switch(many):
            start = time.perf_counter()
            xdotool("mousemove", "--window", win, 125, 82 if many else 143, "click", "--delay", 0, 1)
            try:
                poll(lambda: painted_many(many))
            except AssertionError as error:
                image(display, win, 0, 0, 1024, 700).save(output / "switch-failure.png")
                raise AssertionError(f"Expected {'Maya' if many else 'Bike'} to paint") from error
            elapsed = (time.perf_counter() - start) * 1000
            time.sleep(.23)
            return elapsed

        cold_first = switch(True)
        poll(lambda: events("download"))
        cold_with_preview = switch(False)
        first_count = len(events("read"))
        revisit = [switch(index % 2 == 0) for index in range(12)]
        time.sleep(.95)
        repeat_reads = len(events("read")) - first_count

        # Compare the first changed frame with a settled capture at the saved offset.
        # Keep the cursor, scroll thumb and floating button outside the comparison region.
        def reading_area():
            return image(display, win, 334, 57, 284, 563)

        switch(True)
        at_bottom = reading_area()
        at_bottom.save(output / "scroll-original-bottom.png")
        xdotool("mousemove", "--window", win, 743, 310, "click", "--repeat", 3, "--delay", 36, 4)
        time.sleep(.24)
        saved = reading_area()
        assert saved.tobytes() != at_bottom.tobytes()
        saved.save(output / "scroll-before-switch.png")
        switch(False)
        another = reading_area()
        xdotool("mousemove", "--window", win, 125, 82, "click", "--delay", 0, 1)

        def first_changed():
            frame = reading_area()
            return frame if not same_viewport(frame, another) else None

        def same_viewport(first, second):
            difference = ImageChops.difference(first, second)
            # Subpixel glyph antialiasing can differ at the clip boundary.
            colors = difference.getcolors(20000) or []
            matching = next((n for n, color in colors if color == (0, 0, 0)), 0)
            return first.width * first.height - matching <= 24

        first = poll(first_changed)
        first.save(output / "scroll-first-frame.png")
        sampled = [first]
        for _ in range(16):
            time.sleep(.008)
            sampled.append(reading_area())
        restored = all(same_viewport(frame, saved) for frame in sampled)
        if not baseline:
            assert restored, "The restored chat moved or showed a different scroll position"
            image(display, win, 0, 0, 1024, 700).save(output / "jump-latest.png")
            xdotool("mousemove", "--window", win, 673, 602, "click", "--delay", 0, 1)
            try:
                poll(lambda: same_viewport(reading_area(), at_bottom), timeout=2)
            except AssertionError:
                image(display, win, 0, 0, 1024, 700).save(output / "after-jump-failure.png")
                raise
        if large and not baseline:
            # Open enough distinct chats to evict Maya's view while keeping her messages cached.
            xdotool("mousemove", "--window", win, 743, 310, "click", "--repeat", 3, "--delay", 36, 4)
            time.sleep(.24)
            before_eviction = reading_area()
            for y in (207, 268, 327, 386, 445, 504, 563, 622, 675):
                xdotool("mousemove", "--window", win, 125, y, "click", "--delay", 0, 1)
                time.sleep(.06)
            switch(True)
            assert same_viewport(before_eviction, reading_area()), "An evicted chat lost its scroll position"
        switch(False)

        if large:
            app_log = output / "app.log"
            completed = app_log.read_text().count("sidebar-sync-worker")
            before_refresh = len(events("chats"))
            xdotool("mousemove", "--window", win, 254, 30, "click", "--delay", 0, 1)
            poll(lambda: len(events("chats")) > before_refresh)
            xdotool("key", "ctrl+comma")
            time.sleep(.12)
            xdotool("mousemove", "--window", win, 220, 190, "click", "--delay", 0, 1)
            time.sleep(.4)
            image(display, win, 0, 0, 1024, 700).save(output / "archive-during-sync.png")
            assert "archive-open-ui" in app_log.read_text()
            poll(lambda: app_log.read_text().count("sidebar-sync-worker") > completed)
            time.sleep(.15)
            image(display, win, 0, 0, 1024, 700).save(output / "archive-after-sync.png")
            xdotool("key", "Escape")

        stop(fixture)
        stop(app)
        start = time.perf_counter()
        app, win = open_app("offline.log")
        poll(lambda: painted_many(False))
        offline = (time.perf_counter() - start) * 1000
        print(f"Cold first {cold_first:.0f} ms, cold with media pending {cold_with_preview:.0f} ms")
        print(f"12 warm switches, median {statistics.median(revisit):.0f} ms, "
              f"max {max(revisit):.0f} ms, {repeat_reads} additional message requests")
        print(f"Offline restart showed the last thread in {offline:.0f} ms")
        print(f"First frame at the saved scroll position stayed stable: {restored}")
        (output / "timings.json").write_text(json.dumps({
            "cold_first_ms": cold_first, "cold_with_media_pending_ms": cold_with_preview,
            "warm_ms": revisit, "warm_requests": repeat_reads, "offline_start_ms": offline,
            "saved_scroll_first_frame_stable": restored,
        }, indent=2))
        if not baseline:
            assert repeat_reads == 0, repeat_reads
            assert max(revisit) < 300, revisit


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=Path("target/debug/bluebubbles-native"))
    parser.add_argument("--baseline", action="store_true", help="Measure an older build without regression assertions")
    parser.add_argument("--large", action="store_true", help="Exercise archive navigation while syncing 2,905 chats")
    args = parser.parse_args()
    directory = Path(tempfile.mkdtemp(prefix="bluebubbles-native-switch."))
    print(f"Fixture output in {directory}", flush=True)
    exercise(args.binary.resolve(), directory, args.baseline, args.large)
