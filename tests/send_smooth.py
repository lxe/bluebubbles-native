"""Verify sending and confirming a message has zero geometry jump, dark flash, or widget teardown."""

import argparse
from contextlib import ExitStack
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time

from thread_switch import close_display, image, open_display, poll, stop, xdotool


def bubble_metrics(img):
    """Return (x0, x1, y0, y1, mean_luma) for the bottom-most outgoing blue bubble."""
    w, h = img.size
    # Sample bottom outgoing region (right half, just above composer).
    px = img.load()
    xs, ys, lumas = [], [], []
    # Background canvas is dark (~17, 24, 31); outgoing blue bubble (pending or solid) has b >= 45 and b - r >= 12.
    for y in range(h - 90, h - 10):
        for x in range(w - 280, w - 12):
            r, g, b = px[x, y]
            if b >= 45 and b - r >= 12:
                xs.append(x)
                ys.append(y)
                lumas.append(0.2126 * r + 0.7152 * g + 0.0722 * b)
    if len(xs) < 400:
        return None
    return (min(xs), max(xs), min(ys), max(ys), sum(lumas) / len(lumas))


def test(binary, output):
    with ExitStack() as stack:
        def start(command, name, env=None):
            log = stack.enter_context((output / f"{name}.log").open("w"))
            proc = subprocess.Popen(command, env=env, stdout=log, stderr=log)
            stack.callback(stop, proc)
            return proc

        display = open_display(None)
        assert display, "Use an isolated Xvfb display"
        stack.callback(close_display, display)
        start(["openbox"], "wm")

        ready, receipts = output / "port", output / "requests.jsonl"
        start(
            [
                "python3",
                "tests/fixture.py",
                "--realtime",
                "--send-delay",
                "0.75",
                "--ready",
                str(ready),
                "--receipts",
                str(receipts),
            ],
            "fixture",
        )
        url = (
            "http://127.0.0.1:"
            + poll(lambda: ready.read_text() if ready.exists() else "")
            + "/"
        )

        env = dict(
            os.environ,
            GDK_BACKEND="x11",
            GSK_RENDERER="cairo",
            GTK_A11Y="none",
            GDK_SCALE="1",
            ADW_DEBUG_COLOR_SCHEME="prefer-dark",
            G_DEBUG="fatal-criticals",
            RUST_BACKTRACE="1",
            BLUEBUBBLES_NATIVE_PROFILE="1",
            XDG_CONFIG_HOME=str(output / "config"),
            XDG_CACHE_HOME=str(output / "cache"),
            BLUEBUBBLES_NATIVE_SERVER=url,
            BLUEBUBBLES_NATIVE_PASSWORD="fixture-secret",
        )
        config = Path(env["XDG_CONFIG_HOME"]) / "bluebubbles-native" / "preferences.json"
        config.parent.mkdir(parents=True)
        config.write_text(json.dumps({"server": url, "sidebar_width": 316}))

        start([str(binary)], "app", env)
        win = int(
            xdotool("search", "--sync", "--onlyvisible", "--name", "^BlueBubbles Native$")
            .splitlines()[0]
        )
        xdotool("windowactivate", "--sync", win)

        def events(kind):
            if not receipts.exists():
                return []
            return [
                event
                for row in receipts.read_text().splitlines()
                if (event := json.loads(row))["event"] == kind
            ]

        def click(x, y):
            xdotool("mousemove", "--window", win, x, y, "click", "--delay", 0, 1)

        poll(lambda: events("socket-connected"))
        click(120, 85)
        poll(lambda: events("read"))
        time.sleep(0.35)

        # Send a message and immediately sample the pending bubble at high frequency
        # across server confirmation to ensure zero geometry shift, zero disappear/rebind
        # flash, and monotonic brightening from pending -> confirmed.
        click(460, 662)
        xdotool("type", "--clearmodifiers", "--delay", 0, "Smooth send check")
        xdotool("key", "Return")

        # Wait until the pending bubble paints (still before the 0.75s server reply).
        baseline = poll(lambda: bubble_metrics(image(display, win, 320, 520, 704, 118)))
        assert not events("text-reply"), "Server replied before baseline pending capture"

        samples = [baseline]
        deadline = time.monotonic() + 1.5
        confirmed_seen_at = None
        while time.monotonic() < deadline:
            m = bubble_metrics(image(display, win, 320, 520, 704, 118))
            assert m is not None, "Bubble vanished mid-confirmation (ListView teardown flash)"
            samples.append(m)
            if confirmed_seen_at is None and events("text-reply"):
                confirmed_seen_at = time.monotonic()
            if confirmed_seen_at is not None and time.monotonic() - confirmed_seen_at >= 0.45:
                break
            time.sleep(0.016)

        assert confirmed_seen_at is not None, "Server did not confirm message"
        bx0, bx1, by0, by1, base_luma = baseline
        for idx, (x0, x1, y0, y1, luma) in enumerate(samples):
            assert (x0, x1, y0, y1) == (bx0, bx1, by0, by1), (
                f"Bubble geometry shifted on frame {idx}: {(x0, x1, y0, y1)} != {(bx0, bx1, by0, by1)}"
            )
            assert luma >= base_luma - 2.5, (
                f"Bubble flashed darker than pending on frame {idx}: luma={luma:.1f} < base={base_luma:.1f}"
            )

        final_luma = samples[-1][4]
        assert final_luma > base_luma + 8.0, (
            f"Bubble did not brighten to confirmed state: base={base_luma:.1f}, final={final_luma:.1f}"
        )
        assert "send-confirm-motion" in (output / "app.log").read_text()
        print(
            f"Smooth send confirmation passed ({len(samples)} frames, locked bounds "
            f"{(bx0, bx1, by0, by1)}, luma {base_luma:.1f} -> {final_luma:.1f})."
        )


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=Path("target/debug/bluebubbles-native"))
    args = parser.parse_args()
    directory = Path(tempfile.mkdtemp(prefix="bluebubbles-send-smooth."))
    test(args.binary.resolve(), directory)
