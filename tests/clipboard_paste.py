"""Verify pasting images, videos, file URIs, and plain text from the clipboard in Xvfb."""

import argparse
from contextlib import ExitStack
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

from PIL import Image
from thread_switch import open_display, close_display, poll, stop, xdotool


CLIPBOARD_HELPER = r"""
import json, sys, gi
gi.require_version("Gtk", "4.0")
from gi.repository import Gdk, GLib, Gtk

spec = json.loads(sys.argv[1])
ready_file = sys.argv[2]

Gtk.init()
display = Gdk.Display.get_default()
clipboard = display.get_clipboard()
providers = []
for entry in spec:
    if "file" in entry:
        data = open(entry["file"], "rb").read()
    else:
        data = entry["text"].encode("utf-8")
    providers.append(Gdk.ContentProvider.new_for_bytes(entry["mime"], GLib.Bytes.new(data)))

provider = providers[0] if len(providers) == 1 else Gdk.ContentProvider.new_union(providers)
assert clipboard.set_content(provider), "clipboard.set_content failed"
open(ready_file, "w").write("ready")
GLib.MainLoop().run()
"""


def sample_png_bytes():
    buf = io.BytesIO()
    Image.new("RGB", (64, 48), (48, 128, 224)).save(buf, format="PNG")
    return buf.getvalue()


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

        clip_proc = None

        def set_clipboard(name, spec):
            nonlocal clip_proc
            if clip_proc is not None:
                stop(clip_proc)
                clip_proc = None
            marker = output / f"clip-{name}.ready"
            clip_proc = start(
                [sys.executable, "-c", CLIPBOARD_HELPER, json.dumps(spec), str(marker)],
                f"clip-{name}",
                env,
            )
            poll(lambda: marker.exists())
            time.sleep(0.12)

        # Open first conversation and wait for messages to load.
        poll(lambda: events("chats"))
        click(120, 85)
        poll(lambda: events("read"))
        time.sleep(0.25)

        # 1. Paste raw image/png from clipboard in the composer.
        png_path = output / "clip.png"
        png_bytes = sample_png_bytes()
        png_path.write_bytes(png_bytes)
        set_clipboard("png", [{"mime": "image/png", "file": str(png_path)}])
        click(460, 662)
        time.sleep(0.1)
        xdotool("key", "ctrl+v")
        time.sleep(0.35)
        xdotool("key", "Return")
        uploads = poll(lambda: events("upload") if len(events("upload")) >= 1 else None)
        assert uploads[0]["name"].startswith("pasted-image-"), uploads[0]
        assert uploads[0]["name"].endswith(".png"), uploads[0]
        assert uploads[0]["size"] == len(png_bytes), (uploads[0], len(png_bytes))

        # 2. Paste raw video/mp4 from clipboard.
        mp4_path = output / "clip.mp4"
        mp4_bytes = b"\x00\x00\x00\x18ftypmp42\x00\x00\x00\x00mp42isom" + b"\xab" * 4096
        mp4_path.write_bytes(mp4_bytes)
        set_clipboard("mp4", [{"mime": "video/mp4", "file": str(mp4_path)}])
        click(460, 662)
        time.sleep(0.1)
        xdotool("key", "ctrl+v")
        time.sleep(0.35)
        xdotool("key", "Return")
        uploads = poll(lambda: events("upload") if len(events("upload")) >= 2 else None)
        assert uploads[1]["name"].startswith("pasted-video-"), uploads[1]
        assert uploads[1]["name"].endswith(".mp4"), uploads[1]
        assert uploads[1]["size"] == len(mp4_bytes), (uploads[1], len(mp4_bytes))

        # 3. Paste a file-manager copied video (text/uri-list + text/plain path)
        #    while focus is on the timeline (not yet in the composer).
        named_video = output / "holiday clip.mp4"
        named_bytes = b"\x00\x00\x00\x18ftypisom\x00\x00\x02\x00isomiso2" + b"\xcd" * 8192
        named_video.write_bytes(named_bytes)
        set_clipboard(
            "file-uri",
            [
                {"mime": "text/uri-list", "text": named_video.as_uri() + "\r\n"},
                {"mime": "text/plain;charset=utf-8", "text": str(named_video)},
            ],
        )
        click(650, 260)  # Focus timeline area, outside composer editor.
        time.sleep(0.12)
        xdotool("key", "ctrl+v")
        time.sleep(0.35)
        xdotool("key", "Return")
        uploads = poll(lambda: events("upload") if len(events("upload")) >= 3 else None)
        assert uploads[2]["name"] == "holiday clip.mp4", uploads[2]
        assert uploads[2]["size"] == len(named_bytes), (uploads[2], len(named_bytes))
        assert not events("text"), f"File paste should not have sent text path: {events('text')}"

        # 4. Paste text/uri-list containing only an https:// URL + text/plain ->
        #    falls back cleanly to pasting text into the composer.
        set_clipboard(
            "web-url",
            [
                {"mime": "text/uri-list", "text": "https://example.org/demo\r\n"},
                {"mime": "text/plain;charset=utf-8", "text": "https://example.org/demo"},
            ],
        )
        click(460, 662)
        time.sleep(0.1)
        xdotool("key", "ctrl+v")
        time.sleep(0.3)
        xdotool("key", "Return")
        texts = poll(lambda: events("text") if len(events("text")) >= 1 else None)
        assert texts[0]["text"] == "https://example.org/demo", texts[0]

        # 5. Regular text/plain paste still works normally.
        set_clipboard(
            "plain-text",
            [{"mime": "text/plain;charset=utf-8", "text": "Hello from clipboard"}],
        )
        click(460, 662)
        time.sleep(0.1)
        xdotool("key", "ctrl+v")
        time.sleep(0.25)
        xdotool("key", "Return")
        texts = poll(lambda: events("text") if len(events("text")) >= 2 else None)
        assert texts[1]["text"] == "Hello from clipboard", texts[1]
        assert len(events("upload")) == 3, events("upload")

        print("Clipboard paste (PNG, raw MP4, file-URI video + text/plain, web URL fallback, plain text) passed.")


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=Path("target/debug/bluebubbles-native"))
    args = parser.parse_args()
    directory = Path(tempfile.mkdtemp(prefix="bluebubbles-native-clipboard."))
    test(args.binary.resolve(), directory)
