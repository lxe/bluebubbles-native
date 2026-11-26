"""Exercise live messages and native notifications on an isolated X11 desktop."""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
from urllib.request import Request, urlopen

from gi.repository import Gio, GLib
from PIL import Image

from tray import poll


NOTIFICATIONS = "org.freedesktop.Notifications"
PATH = "/org/freedesktop/Notifications"
MAYA = "iMessage;-;+15550100100"
BIKE = "iMessage;-;+15550100101"
XML = f"""<node><interface name="{NOTIFICATIONS}">
  <method name="Notify">
    <arg type="s" direction="in"/><arg type="u" direction="in"/><arg type="s" direction="in"/>
    <arg type="s" direction="in"/><arg type="s" direction="in"/><arg type="as" direction="in"/>
    <arg type="a{{sv}}" direction="in"/><arg type="i" direction="in"/><arg type="u" direction="out"/>
  </method>
  <method name="CloseNotification"><arg type="u" direction="in"/></method>
  <method name="Click"><arg type="u" direction="in"/></method>
  <signal name="ActionInvoked"><arg type="u"/><arg type="s"/></signal>
</interface></node>"""


def serve_notifications(output):
    bus = Gio.bus_get_sync(Gio.BusType.SESSION)
    count = 0

    def method(_, _sender, _path, _interface, name, arguments, invocation):
        nonlocal count
        args = arguments.unpack()
        if name == "Notify":
            count += 1
            identifier = args[1] or count
            with (output / "notifications.jsonl").open("a") as file:
                print(json.dumps({"id": identifier, "title": args[3], "body": args[4], "actions": args[5]}), file=file)
            invocation.return_value(GLib.Variant("(u)", (identifier,)))
        elif name == "Click":
            bus.emit_signal(None, PATH, NOTIFICATIONS, "ActionInvoked", GLib.Variant("(us)", (args[0], "default")))
            invocation.return_value(GLib.Variant("()", ()))
        else:
            invocation.return_value(GLib.Variant("()", ()))

    bus.register_object(PATH, Gio.DBusNodeInfo.new_for_xml(XML).interfaces[0], method, None, None)
    Gio.bus_own_name_on_connection(bus, NOTIFICATIONS, Gio.BusNameOwnerFlags.NONE, None, None)
    GLib.MainLoop().run()


def run(output):
    env = dict(os.environ, XDG_CONFIG_HOME=str(output / "config"), XDG_CACHE_HOME=str(output / "cache"),
               GDK_BACKEND="x11", GSK_RENDERER="cairo", GTK_A11Y="none", G_DEBUG="fatal-criticals",
               ADW_DEBUG_COLOR_SCHEME="prefer-dark")
    processes = []

    def launch(name, args):
        with (output / f"{name}.log").open("w") as log:
            process = subprocess.Popen(args, env=env, stdout=log, stderr=log)
        processes.append(process)
        return process

    def rows(name):
        file = output / f"{name}.jsonl"
        return [json.loads(line) for line in file.read_text().splitlines()] if file.exists() else []

    def count(kind):
        return sum(event["event"] == kind for event in rows("requests"))

    def input_event(*args):
        subprocess.run(["xdotool", *map(str, args)], check=True, timeout=5)

    def visible():
        result = subprocess.run(["xdotool", "search", "--onlyvisible", "--name", "^BlueBubbles Native$"],
                                capture_output=True, text=True)
        return next(iter(result.stdout.splitlines()), None)

    def capture_text(name, region=None):
        image = output / f"{name}.png"
        subprocess.run(["import", "-window", win, str(image)], check=True, timeout=5)
        if region is not None:
            crop = Image.open(image).crop(region)
            image = output / f"{name}-crop.png"
            crop.resize((crop.width * 2, crop.height * 2)).save(image)
        return subprocess.check_output(["tesseract", str(image), "stdout", "--psm", "11"],
                                       text=True, stderr=subprocess.DEVNULL, timeout=5)

    def post(path, data):
        with urlopen(Request(url + path + "?guid=fixture-secret", data=json.dumps(data).encode()), timeout=3) as response:
            assert response.status == 200

    def message(chat, identifier, text, *, outgoing=False, publish=True, event="new-message", stamp=None):
        data = {
            "guid": identifier, "text": text, "isFromMe": outgoing,
            "dateCreated": stamp or int(time.time() * 1000),
        }
        if event == "updated-message":
            data["dateEdited"] = int(time.time() * 1000)
        post("fixture/event", {"chat": chat, "event": event, "publish": publish, "message": data})

    def click_notification(identifier):
        bus = Gio.bus_get_sync(Gio.BusType.SESSION)
        bus.call_sync(NOTIFICATIONS, PATH, NOTIFICATIONS, "Click", GLib.Variant("(u)", (identifier,)),
                      None, Gio.DBusCallFlags.NONE, 2000, None)

    try:
        launch("wm", ["openbox"])
        launch("daemon", [sys.executable, __file__, "--daemon", str(output)])
        port = output / "port"
        launch("fixture", [sys.executable, "tests/fixture.py", "--realtime",
                           "--query-delay", "0.7", "--ready", str(port),
                           "--receipts", str(output / "requests.jsonl")])
        url = "http://127.0.0.1:" + poll(lambda: port.read_text() if port.exists() else "", "Server did not start") + "/"
        env.update(BLUEBUBBLES_NATIVE_SERVER=url, BLUEBUBBLES_NATIVE_PASSWORD="fixture-secret")
        launch("app", ["./target/debug/bluebubbles-native"])
        win = poll(visible, "The app window did not open")
        input_event("windowactivate", "--sync", win)
        poll(lambda: count("chats") and count("socket-connected"), "Initial connection did not finish")
        poll(lambda: "Maya Chen" in capture_text("ready", (5, 60, 315, 125)),
             "Initial conversation list did not paint")
        input_event("mousemove", "--window", win, 120, 85, "click", 1)
        poll(lambda: count("read"), "Opening the first conversation did not fetch messages")
        time.sleep(.25)
        reads, queries = count("read"), count("chats")

        stamp = int(time.time() * 1000)
        message(MAYA, "live-visible", "Pushed without polling", stamp=stamp)
        poll(lambda: capture_text("push").count("Pushed without polling") >= 2, "The push did not update the thread and sidebar")
        message(MAYA, "live-visible", "Edited through push", event="updated-message", stamp=stamp)
        poll(lambda: capture_text("edit").count("Edited through push") >= 2, "The edit did not update the thread and sidebar")
        cached_heads = next((output / "cache").rglob("live-chat-heads.json"))
        assert json.loads(cached_heads.read_text())[MAYA]["preview"] == "Edited through push"
        assert count("read") == reads and count("chats") == queries, rows("requests")
        assert rows("notifications") == [], "A visible thread produced a notification"

        message(BIKE, "outgoing", "Sent from another device", outgoing=True)
        message(BIKE, "notification", "Incoming in another chat")
        first = poll(lambda: next(iter(rows("notifications")), None), "The desktop daemon did not receive a notification")
        assert (first["title"], first["body"]) == ("Bike Workshop", "Incoming in another chat"), first
        assert "default" in first["actions"], first
        message(BIKE, "notification", "Incoming in another chat")
        time.sleep(.25)
        assert len(rows("notifications")) == 1
        click_notification(first["id"])
        poll(lambda: "Incoming in another chat" in capture_text("opened", (330, 57, 1015, 625)),
             "The notification did not open the conversation")

        input_event("windowminimize", win)
        poll(lambda: visible() is None, "The app did not minimize")
        message(BIKE, "while-hidden", "Arrived while minimized")
        poll(lambda: len(rows("notifications")) == 2, "The minimized app missed a notification")
        click_notification(rows("notifications")[-1]["id"])
        assert poll(visible, "Clicking the notification did not reopen the window") == win
        poll(lambda: "Arrived while minimized" in capture_text("reopened", (330, 57, 1015, 625)),
             "The restored thread missed the message")

        # Allow one idle poll to carry a heartbeat before disconnecting the fixture socket.
        poll(lambda: count("socket-pong"), "The app did not answer an Engine.IO heartbeat")
        # The REST safety net recovers new messages while the Socket.IO transport is unavailable.
        post("fixture/live", {"enabled": False})
        message(MAYA, "missed", "Recovered after disconnect", publish=False)
        cutoff = time.monotonic() + 22
        while time.monotonic() < cutoff and count("chats") == queries:
            time.sleep(.04)
        assert count("chats") > queries, "The REST fallback did not run"
        image = output / "fallback.png"
        subprocess.run(["import", "-window", win, str(image)], check=True, timeout=5)
        control = (237, 13, 271, 47)
        assert Image.open(image).convert("RGB").crop(control).tobytes() == (
            Image.open(output / "reopened.png").convert("RGB").crop(control).tobytes()
        ), "A background REST sync animated the toolbar"
        poll(lambda: len(rows("notifications")) == 3, "The REST fallback did not notify")
        assert rows("notifications")[-1]["body"] == "Recovered after disconnect", rows("notifications")
        post("fixture/live", {"enabled": True})
        cutoff = time.monotonic() + 20
        while time.monotonic() < cutoff and count("socket-connected") < 2:
            time.sleep(.10)
        assert count("socket-connected") >= 2, "The socket did not reconnect"
        message(BIKE, "after-reconnect", "Live after reconnect")
        poll(lambda: "Live after reconnect" in capture_text("reconnected", (330, 57, 1015, 625)),
             "Live delivery did not resume after reconnecting")
        print("Push, edits, native notifications, click-through, REST recovery, and reconnect passed.")
    finally:
        for process in reversed(processes):
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=3)


if __name__ == "__main__":
    if sys.argv[1:2] == ["--daemon"]:
        serve_notifications(Path(sys.argv[2]))
    else:
        directory = Path(tempfile.mkdtemp(prefix="bluebubbles-realtime."))
        print(f"Live message test output: {directory}", flush=True)
        run(directory)
