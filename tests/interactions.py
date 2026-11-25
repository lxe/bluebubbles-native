"""Exercise the viewer, resizing, optimistic sends and live scrolling in Xvfb.

Run with xvfb-run -a -s '-screen 0 1600x1100x24' dbus-run-session --
python3 tests/interactions.py. Use --profile-only to compare older binaries.
"""

import argparse
from contextlib import ExitStack
import json
import os
from pathlib import Path
import re
import statistics
import subprocess
import tempfile
import time
from urllib.request import Request, urlopen

from thread_switch import close_display, image, open_display, poll, stop, xdotool


MAYA = "iMessage;-;+15550100100"
BLUE = (32, 126, 205)
ORANGE = (210, 93, 38)


def loop_gaps(log):
    return list(map(float, re.findall(r"profile main-loop gap: ([.\d]+) ms", log)))


def test(binary, output, profile_only, renderer):
    with ExitStack() as stack:
        def start(command, name, env=None):
            log = stack.enter_context((output / (name + ".log")).open("w"))
            proc = subprocess.Popen(command, env=env, stdout=log, stderr=log)
            stack.callback(stop, proc)
            return proc

        display = open_display(None)
        assert display, "Use an isolated Xvfb display"
        stack.callback(close_display, display)
        start(["openbox"], "wm")
        ready, receipts = output / "port", output / "requests.jsonl"
        start(["python3", "tests/fixture.py", "--realtime", "--large-image", "--send-delay", "1.65",
               "--reject-text", "Restore this draft", "--message-delay", "0.40",
               "--preview-delay", "0.48", "--ready", str(ready), "--receipts", str(receipts)], "fixture")
        url = "http://127.0.0.1:" + poll(lambda: ready.read_text() if ready.exists() else "") + "/"
        env = dict(os.environ, GDK_BACKEND="x11", GSK_RENDERER=renderer, GTK_A11Y="none",
                   GDK_SCALE="1", ADW_DEBUG_COLOR_SCHEME="prefer-dark", G_DEBUG="fatal-criticals",
                   RUST_BACKTRACE="1", BLUEBUBBLES_NATIVE_PROFILE="1",
                   XDG_CONFIG_HOME=str(output / "config"), XDG_CACHE_HOME=str(output / "cache"),
                   BLUEBUBBLES_NATIVE_SERVER=url, BLUEBUBBLES_NATIVE_PASSWORD="fixture-secret")
        config = Path(env["XDG_CONFIG_HOME"]) / "bluebubbles-native" / "preferences.json"
        config.parent.mkdir(parents=True)
        config.write_text(json.dumps({"server": url, "sidebar_width": 316}))

        def app_window(name):
            app = start([str(binary)], name, env)
            win = int(xdotool("search", "--sync", "--onlyvisible", "--name", "^BlueBubbles Native$").splitlines()[0])
            xdotool("windowactivate", "--sync", win)
            return app, win

        app, win = app_window("app")

        def events(name):
            if not receipts.exists():
                return []
            return [event for row in receipts.read_text().splitlines()
                    if (event := json.loads(row))["event"] == name]

        def click(x, y):
            xdotool("mousemove", "--window", win, x, y, "click", "--delay", 0, 1)

        def capture(bounds=(0, 0, 1024, 700)):
            return image(display, win, *bounds)

        def preview(image):
            return any(n > 2 and 20 < r < 110 and 58 < g < 170 and 132 < b < 220
                       for n, (r, g, b) in image.getcolors(50000))

        def lightbox_preview(image):
            return sum(n for n, (r, g, b) in (image.getcolors(50000) or [])
                       if 66 < r < 104 and 65 < g < 147 and 174 < b < 220) > 300

        def palette(image):
            pixels = image.getcolors(50000) or []
            return any(n > 250 and color in (BLUE, ORANGE) for n, color in pixels)

        def ocr(name, bounds):
            region = capture(bounds)
            path = output / (name + ".png")
            region.resize((region.width * 2, region.height * 2)).save(path)
            threshold = path.with_suffix(".pgm")
            region.resize((region.width * 2, region.height * 2)).convert("L").point(
                lambda value: 0 if value > 135 else 255
            ).save(threshold)
            return subprocess.check_output(["tesseract", str(threshold), "stdout", "--psm", "11"],
                                           text=True, stderr=subprocess.DEVNULL, timeout=5)

        def input_text(text):
            click(425, 662)
            xdotool("type", "--clearmodifiers", "--delay", 0, text)
            xdotool("key", "Return")

        def post(data):
            request = Request(url + "fixture/event?guid=fixture-secret", json.dumps(data).encode())
            with urlopen(request, timeout=3) as response:
                assert response.status == 200

        poll(lambda: events("socket-connected"))
        click(120, 85)
        poll(lambda: events("read"))
        poll(lambda: preview(capture((372, 560, 100, 42))))
        capture().save(output / "thread.png")

        profile_log = output / "app.log"
        viewer_start = len(profile_log.read_text())
        viewer_area = (613, 253, 135, 210)
        viewer_first, viewer_original = [], []
        before_downloads = sum(event["original"] for event in events("download"))
        for index in range(5):
            assert not lightbox_preview(capture(viewer_area)) and not palette(capture(viewer_area))
            start_time = time.perf_counter()
            click(470, 570)
            poll(lambda: lightbox_preview(capture(viewer_area)) or palette(capture(viewer_area)))
            viewer_first.append((time.perf_counter() - start_time) * 1000)
            poll(lambda: palette(capture(viewer_area)))
            viewer_original.append((time.perf_counter() - start_time) * 1000)
            if index == 0:
                capture().save(output / "viewer.png")
            xdotool("key", "Escape")
            poll(lambda: not lightbox_preview(capture(viewer_area)) and not palette(capture(viewer_area)))
            time.sleep(.34)
        assert sum(event["original"] for event in events("download")) == before_downloads + 1

        viewer_log = profile_log.read_text()[viewer_start:]
        resize_start = len(profile_log.read_text())

        # Resize both the app and its sidebar with loaded media visible.
        start_time = time.perf_counter()
        for width in (880, 1110, 970, 1200, 1024) * 3:
            xdotool("windowsize", "--sync", win, width, 700)
            time.sleep(.025)
        xdotool("mousemove", "--window", win, 320, 357, "mousedown", 1)
        for x in list(range(325, 481, 13)) + list(range(480, 319, -13)):
            xdotool("mousemove", "--window", win, x, 357)
            time.sleep(.018)
        xdotool("mouseup", 1)
        time.sleep(.35)
        elapsed = (time.perf_counter() - start_time) * 1000
        capture().save(output / "resize.png")
        metrics = {
            "viewer_preview_ms": viewer_first,
            "viewer_full_ms": viewer_original,
            "scripted_resize_ms": elapsed,
            "viewer_main_loop_gaps_ms": loop_gaps(viewer_log),
            "resize_main_loop_gaps_ms": loop_gaps(profile_log.read_text()[resize_start:]),
        }

        if not profile_only:
            # The server holds each POST for 1.65 seconds. Two messages should paint
            # before the first HTTP response, then resolve to one bubble each.
            input_text("Quick brown fox")
            input_text("Follow up before sync")
            time.sleep(.18)
            painted = ocr("optimistic", (590, 380, 420, 259))
            assert "Quick brown fox" in painted and "Follow up before sync" in painted, painted
            assert len(events("text")) == 2 and not events("text-reply"), (events("text"), events("text-reply"))
            colors = capture((920, 594, 70, 20)).getcolors(1500)
            assert any(n > 4 and abs(r - 30) < 5 and abs(g - 51) < 5 and abs(b - 69) < 7
                       for n, (r, g, b) in colors), colors
            assert "Sending" not in painted, painted
            first_shimmer = capture((855, 595, 136, 25)).tobytes()
            time.sleep(.14)
            assert capture((855, 595, 136, 25)).tobytes() != first_shimmer
            poll(lambda: len(events("text-reply")) == 2, timeout=7)
            time.sleep(.35)
            painted = ocr("confirmed", (590, 380, 420, 259))
            assert "send-confirm-motion" in profile_log.read_text()
            assert painted.count("Quick brown fox") == 1, painted
            assert painted.count("Follow up before sync") == 1 and "Sending" not in painted, painted

            input_text("Restore this draft")
            poll(lambda: len(events("text-reply")) == 3)
            time.sleep(.30)
            painted = ocr("failed", (590, 380, 420, 259))
            assert "Restore this draft" in painted, painted
            failure = capture((885, 597, 118, 30))
            assert any(n > 6 and r > 180 and r - g > 55 and b > 130
                       for n, (r, g, b) in failure.getcolors(4000))
            # The latest failure is the right-aligned bubble at the bottom of the timeline.
            xdotool("mousemove", "--window", win, 930, 586, "click", "--delay", 0, 3)
            time.sleep(.22)
            capture().save(output / "restore-menu.png")
            xdotool("key", "End", "Return")
            time.sleep(.18)
            click(690, 648)
            time.sleep(.28)
            assert "Restore this draft" in ocr("restored", (375, 643, 620, 35))

            # Rewind the timeline, then push a media-only message. The thumbnail
            # arrives later and changes the row height, so check the bottom afterwards.
            xdotool("mousemove", "--window", win, 743, 330, "click", "--repeat", 4, "4")
            time.sleep(.28)
            assert "Follow up before sync" not in ocr("rewound", (330, 65, 665, 555))
            post({"chat": MAYA, "message": {"guid": "live-photo", "text": "Fresh trail photo",
                 "dateCreated": int(time.time() * 1000),
                 "attachments": [{"guid": "live-fixture-image", "mimeType": "image/png",
                                  "transferName": "Live trail.png"}]}})
            poll(lambda: any(event["guid"] == "live-fixture-image" for event in events("download")))
            poll(lambda: preview(capture((435, 582, 60, 34))))
            capture().save(output / "incoming-at-bottom.png")

            stop(app)
            _, win = app_window("warm")
            poll(lambda: "Fresh trail photo" in ocr("reopened-cache", (320, 384, 683, 246)))
            assert "thread-loading-indicator" not in (output / "warm.log").read_text()
            poll(lambda: len([event for event in events("read") if event["offset"] == 0]) >= 2)
            time.sleep(.5)
            # A scrollbar drag can reach the boundary without an overscroll gesture.
            xdotool("mousemove", "--window", win, 1014, 584)
            time.sleep(.23)
            xdotool("mousedown", 1, "mousemove", "--window", win, 1014, 73, "mouseup", 1)
            time.sleep(.2)
            capture().save(output / "history-boundary.png")
            initial_reads = len([event for event in events("read") if event["offset"] > 0])
            assert initial_reads == 0, events("read")
            assert "Load More Messages" in ocr("load-more", (555, 56, 242, 65))
            click(669, 79)
            poll(lambda: any(event["offset"] > 0 for event in events("read")))

        spans = (output / "app.log").read_text()
        metrics["main_loop_gaps_ms"] = loop_gaps(spans)
        metrics["viewer_frames"] = [list(match) for match in re.findall(
            r"profile viewer-opening-frames: (\d+) frames, max ([.\d]+) ms", spans)]
        (output / "timings.json").write_text(json.dumps(metrics, indent=2))
        print(f"Viewer click to first image: {statistics.median(viewer_first):.1f} ms median, {max(viewer_first):.1f} ms max")
        print(f"Viewer click to higher-resolution image: {viewer_original[0]:.1f} ms cold, "
              f"{statistics.median(viewer_original[1:]):.1f} ms warm median")
        if not profile_only:
            print("Optimistic sends, confirmed sends, failure recovery, incoming photo scroll, cached reopening, and history pagination passed.")


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=Path("target/debug/bluebubbles-native"))
    parser.add_argument("--profile-only", action="store_true")
    parser.add_argument("--renderer", default="cairo")
    args = parser.parse_args()
    directory = Path(tempfile.mkdtemp(prefix="bluebubbles-native-interactions."))
    print(f"Interaction output in {directory}", flush=True)
    test(args.binary.resolve(), directory, args.profile_only, args.renderer)
