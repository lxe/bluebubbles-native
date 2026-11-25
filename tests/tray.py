"""Exercise the GTK window and its D-Bus tray menu on an isolated desktop."""

import csv
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

from gi.repository import Gio, GLib


WATCHER = "org.kde.StatusNotifierWatcher"
ITEM = "org.kde.StatusNotifierItem"
MENU = "com.canonical.dbusmenu"
WATCHER_XML = f"""<node>
  <interface name="{WATCHER}">
    <method name="RegisterStatusNotifierItem"><arg type="s" direction="in"/></method>
    <property name="IsStatusNotifierHostRegistered" type="b" access="read"/>
  </interface>
</node>"""


def mock_watcher(output):
    bus = Gio.bus_get_sync(Gio.BusType.SESSION)

    def register(_, sender, _path, _interface, _method, arguments, invocation):
        address, = arguments.unpack()
        service, path = (sender, address) if address.startswith("/") else (address, "/StatusNotifierItem")
        (output / "registered").write_text(f"{service} {path}")
        invocation.return_value(GLib.Variant("()", ()))

    bus.register_object(
        "/StatusNotifierWatcher", Gio.DBusNodeInfo.new_for_xml(WATCHER_XML).interfaces[0],
        register, lambda *_: GLib.Variant("b", True),
    )
    Gio.bus_own_name_on_connection(bus, WATCHER, Gio.BusNameOwnerFlags.NONE, None, None)
    GLib.MainLoop().run()


def poll(condition, description):
    cutoff = time.monotonic() + 6
    while time.monotonic() < cutoff:
        if value := condition():
            return value
        time.sleep(.04)
    raise AssertionError(description)


def run(output):
    bus = Gio.bus_get_sync(Gio.BusType.SESSION)
    env = dict(os.environ, XDG_CONFIG_HOME=str(output / "config"),
               XDG_CACHE_HOME=str(output / "cache"), GDK_BACKEND="x11",
               GSK_RENDERER="cairo", GTK_A11Y="none", G_DEBUG="fatal-criticals",
               ADW_DEBUG_COLOR_SCHEME="prefer-dark")
    for key in ("BLUEBUBBLES_NATIVE_SERVER", "BLUEBUBBLES_NATIVE_PASSWORD"):
        env.pop(key, None)
    config = output / "config" / "bluebubbles-native" / "preferences.json"
    gtk_settings = output / "config" / "gtk-4.0" / "settings.ini"
    gtk_settings.parent.mkdir(parents=True)
    gtk_settings.write_text("[Settings]\ngtk-decoration-layout=:close\n")
    registration = output / "registered"
    processes = []

    def launch(name, args):
        with (output / f"{name}.log").open("w") as log:
            process = subprocess.Popen(args, env=env, stdout=log, stderr=log)
        processes.append(process)
        return process

    def dbus(service, path, interface, method, signature, values):
        result = bus.call_sync(service, path, interface, method,
                               GLib.Variant(signature, values), None, Gio.DBusCallFlags.NONE, 2000, None)
        return result.unpack()

    def items():
        return registration.read_text().split() if registration.exists() else None

    def visible_window():
        result = subprocess.run(
            ["xdotool", "search", "--onlyvisible", "--name", "^BlueBubbles Native$"],
            capture_output=True, text=True,
        )
        return next(iter(result.stdout.splitlines()), None)

    def input_event(*args):
        subprocess.run(["xdotool", *map(str, args)], check=True, timeout=5)

    def click(x, y):
        input_event("mousemove", "--window", win, x, y, "click", 1)

    def click_label(capture, word):
        image = output / f"{capture}.png"
        time.sleep(.3)
        subprocess.run(["import", "-window", win, str(image)], check=True, timeout=5)
        data = subprocess.check_output(
            ["tesseract", str(image), "stdout", "--psm", "11", "tsv"],
            text=True, stderr=subprocess.DEVNULL, timeout=8,
        )
        row = next(row for row in csv.DictReader(io.StringIO(data), delimiter="\t")
                   if row["text"] == word and 150 < int(row["left"]) < 860)
        click(int(row["left"]) + int(row["width"]) // 2, int(row["top"]) + int(row["height"]) // 2)

    def menu_command(service, label):
        (path,) = dbus(service, item, "org.freedesktop.DBus.Properties", "Get", "(ss)", (ITEM, "Menu"))
        _, layout = dbus(service, path, MENU, "GetLayout", "(iias)", (0, -1, ["label"]))
        entry = next(row[0] for row in layout[2] if row[1].get("label") == label)
        dbus(service, path, MENU, "Event", "(isvu)", (entry, "clicked", GLib.Variant("i", 0), 0))

    def assert_hidden():
        try:
            poll(lambda: visible_window() is None, "BlueBubbles did not hide")
        except AssertionError:
            subprocess.run(["import", "-window", win, str(output / "hide-failure.png")], timeout=5)
            raise
        assert app.poll() is None, "Hiding the window quit the app"
        state = subprocess.check_output(["xprop", "-id", win, "WM_STATE"], text=True)
        assert "Iconic" not in state, state

    try:
        launch("wm", ["openbox"])
        watcher = launch("tray-host", [sys.executable, __file__, "--watcher", str(output)])
        app = launch("app", ["./target/debug/bluebubbles-native"])
        win = poll(visible_window, "BlueBubbles did not open")
        input_event("windowactivate", "--sync", win)
        input_event("key", "ctrl+comma")
        click_label("preferences", "Desktop")
        click_label("desktop", "Minimize")
        service, item = poll(items, "Enabling minimize did not register the tray item")
        assert json.loads(config.read_text())["tray"] == {"minimize": True, "close": False}
        (pixmaps,) = dbus(service, item, "org.freedesktop.DBus.Properties", "Get", "(ss)", (ITEM, "IconPixmap"))
        assert [(w, h, len(data)) for w, h, data in pixmaps] == [(16, 16, 1024), (32, 32, 4096)]
        click_label("minimize-enabled", "Close")
        poll(lambda: json.loads(config.read_text())["tray"] == {"minimize": True, "close": True},
             "Close to Tray preference did not persist")
        input_event("key", "Escape")
        time.sleep(.3)
        click(959, 30)
        assert_hidden()
        dbus(service, item, ITEM, "Activate", "(ii)", (0, 0))
        assert poll(visible_window, "Activating the tray did not reopen the window") == win

        click(995, 30)
        assert_hidden()
        menu_command(service, "Open BlueBubbles")
        assert poll(visible_window, "The tray menu did not reopen the window") == win
        click(995, 30)
        assert_hidden()
        watcher.terminate()
        watcher.wait(timeout=3)
        assert poll(visible_window, "Losing the tray left the window hidden") == win
        registration.unlink()
        watcher = launch("tray-restart", [sys.executable, __file__, "--watcher", str(output)])
        service, item = poll(items, "BlueBubbles did not register with the restarted tray")
        click(995, 30)
        assert_hidden()
        menu_command(service, "Quit")
        assert app.wait(timeout=5) == 0, "Tray Quit did not stop the app"

        registration.unlink()
        app = launch("app-restart", ["./target/debug/bluebubbles-native"])
        win = poll(visible_window, "BlueBubbles did not restart")
        poll(items, "The tray options did not persist after restart")
        click(995, 30)
        assert_hidden()
        watcher.terminate()
        watcher.wait(timeout=3)
        assert poll(visible_window, "Losing the tray left the restarted app hidden") == win
        click(959, 30)
        poll(lambda: visible_window() is None, "The normal minimize fallback did not work")
        state = subprocess.check_output(["xprop", "-id", win, "WM_STATE"], text=True)
        assert "Iconic" in state, state
        input_event("windowactivate", "--sync", win)
        assert poll(visible_window, "The window did not restore after normal minimize") == win
        click(995, 30)
        assert app.wait(timeout=5) == 0, "Closing without a tray did not quit"
        print("Tray preferences, minimize, close, reopen, fallback, and Quit passed.")
    finally:
        for process in reversed(processes):
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=3)


if __name__ == "__main__":
    if sys.argv[1:2] == ["--watcher"]:
        mock_watcher(Path(sys.argv[2]))
    else:
        output = Path(tempfile.mkdtemp(prefix="bluebubbles-tray."))
        print(f"Tray test output: {output}", flush=True)
        run(output)
