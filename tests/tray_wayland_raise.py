"""Verify tray activation brings BlueBubbles Native forward on Wayland."""

import argparse
from contextlib import ExitStack
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

COVER_APP = r"""
import gi
gi.require_version("Gtk", "4.0")
from gi.repository import Gtk

app = Gtk.Application(application_id="co.lxe.CoverWindow")
def on_activate(a):
    w = Gtk.ApplicationWindow(application=a, title="Cover Window")
    w.set_default_size(600, 400)
    w.present()
app.connect("activate", on_activate)
app.run(None)
"""


def poll(condition, description, timeout=6.0):
    cutoff = time.monotonic() + timeout
    while time.monotonic() < cutoff:
        value = condition()
        if value:
            return value
        time.sleep(0.05)
    raise AssertionError(description)


def stop(proc):
    if proc.poll() is None:
        proc.terminate()
        try:
            proc.wait(timeout=3)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait(timeout=3)


def test(binary, output):
    bus = Gio.bus_get_sync(Gio.BusType.SESSION)

    with ExitStack() as stack:
        def start(command, name, env=None):
            log = stack.enter_context((output / f"{name}.log").open("w"))
            proc = subprocess.Popen(command, env=env, stdout=log, stderr=log)
            stack.callback(stop, proc)
            return proc

        runtime = output / "runtime"
        runtime.mkdir(mode=0o700)
        sway_cfg = output / "sway.cfg"
        sway_cfg.write_text(
            'output HEADLESS-1 resolution 1280x800@60Hz\n'
            'for_window [app_id=".*"] floating enable\n'
        )
        sway_env = dict(
            os.environ,
            WLR_BACKENDS="headless",
            WLR_LIBINPUT_NO_DEVICES="1",
            WLR_RENDERER="pixman",
            XDG_RUNTIME_DIR=str(runtime),
        )
        sway_env.pop("DISPLAY", None)
        sway_env.pop("WAYLAND_DISPLAY", None)
        sway_env.pop("HYPRLAND_INSTANCE_SIGNATURE", None)
        start(["sway", "-c", str(sway_cfg)], "sway", sway_env)

        def find_sockets():
            files = os.listdir(runtime)
            sock = next((f for f in files if f.startswith("sway-ipc.") and f.endswith(".sock")), None)
            wdisp = next((f for f in files if f.startswith("wayland-") and not f.endswith(".lock")), None)
            return (str(runtime / sock), wdisp) if sock and wdisp else None

        sock, wdisp = poll(find_sockets, "Headless Sway did not start")
        env = dict(
            sway_env,
            WAYLAND_DISPLAY=wdisp,
            SWAYSOCK=sock,
            GDK_BACKEND="wayland",
            GSK_RENDERER="cairo",
            GTK_A11Y="none",
            G_DEBUG="fatal-criticals",
            ADW_DEBUG_COLOR_SCHEME="prefer-dark",
            XDG_CONFIG_HOME=str(output / "config"),
            XDG_CACHE_HOME=str(output / "cache"),
        )
        for key in ("BLUEBUBBLES_NATIVE_SERVER", "BLUEBUBBLES_NATIVE_PASSWORD"):
            env.pop(key, None)

        config = Path(env["XDG_CONFIG_HOME"]) / "bluebubbles-native" / "preferences.json"
        config.parent.mkdir(parents=True)
        config.write_text(json.dumps({"tray": {"minimize": True, "close": True}}))

        def swaymsg(*args):
            return subprocess.check_output(
                ["swaymsg", "-s", sock, *args], env=env, text=True, timeout=5
            )

        def focused_app():
            tree = json.loads(swaymsg("-t", "get_tree"))

            def walk(node, ws=None):
                if node.get("type") == "workspace":
                    ws = node.get("name")
                if node.get("focused"):
                    return (node.get("app_id") or node.get("name"), ws)
                for child in node.get("nodes", []) + node.get("floating_nodes", []):
                    hit = walk(child, ws)
                    if hit:
                        return hit
                return None

            return walk(tree)

        def dbus(service, path, interface, method, signature, values):
            result = bus.call_sync(
                service,
                path,
                interface,
                method,
                GLib.Variant(signature, values),
                None,
                Gio.DBusCallFlags.NONE,
                2000,
                None,
            )
            return result.unpack()

        registration = output / "registered"
        start(
            [sys.executable, "tests/tray.py", "--watcher", str(output)],
            "watcher",
            env,
        )
        poll(
            lambda: bus.call_sync(
                "org.freedesktop.DBus",
                "/org/freedesktop/DBus",
                "org.freedesktop.DBus",
                "NameHasOwner",
                GLib.Variant("(s)", (WATCHER,)),
                None,
                Gio.DBusCallFlags.NONE,
                1000,
                None,
            ).unpack()[0],
            "Mock StatusNotifierWatcher did not claim bus name",
        )

        app = start([str(binary)], "app", env)
        service, item = poll(
            lambda: registration.read_text().split() if registration.exists() else None,
            "BlueBubbles did not register its Wayland tray item",
        )
        poll(
            lambda: focused_app() == ("co.lxe.BlueBubblesNative", "1"),
            "BlueBubbles did not open on workspace 1",
        )

        # Open a second floating window on top so BlueBubbles is buried/unfocused.
        start([sys.executable, "-c", COVER_APP], "cover", env)
        poll(
            lambda: focused_app() == ("co.lxe.CoverWindow", "1"),
            "Cover window did not take focus",
        )

        # 1. Tray Activate without an xdg_activation_v1 token brings BlueBubbles forward.
        dbus(service, item, ITEM, "Activate", "(ii)", (0, 0))
        poll(
            lambda: focused_app() == ("co.lxe.BlueBubblesNative", "1"),
            f"Tray Activate without token did not raise window (focused={focused_app()})",
        )

        # 2. Focus cover window again, send an unhonored activation token + Activate
        #    (simulates Hyprland misc:focus_on_activate = false).
        swaymsg('[app_id="co.lxe.CoverWindow"]', "focus")
        poll(lambda: focused_app() == ("co.lxe.CoverWindow", "1"), "Cover window did not refocus")
        dbus(service, item, ITEM, "ProvideXdgActivationToken", "(s)", ("ignored-token",))
        dbus(service, item, ITEM, "Activate", "(ii)", (0, 0))
        poll(
            lambda: focused_app() == ("co.lxe.BlueBubblesNative", "1"),
            f"Tray Activate with ignored token did not raise window (focused={focused_app()})",
        )

        # 3. Switch to workspace 2 and click tray -> brings BlueBubbles to workspace 2 & focuses it.
        swaymsg("workspace", "2")
        poll(lambda: focused_app()[1] == "2", "Could not switch to workspace 2")
        dbus(service, item, ITEM, "Activate", "(ii)", (0, 0))
        poll(
            lambda: focused_app() == ("co.lxe.BlueBubblesNative", "2"),
            f"Tray Activate on workspace 2 did not bring window forward (focused={focused_app()})",
        )

        # Clean quit via tray menu.
        (menu_path,) = dbus(
            service, item, "org.freedesktop.DBus.Properties", "Get", "(ss)", (ITEM, "Menu")
        )
        _, layout = dbus(service, menu_path, MENU, "GetLayout", "(iias)", (0, -1, ["label"]))
        quit_id = next(row[0] for row in layout[2] if row[1].get("label") == "Quit")
        dbus(service, menu_path, MENU, "Event", "(isvu)", (quit_id, "clicked", GLib.Variant("i", 0), 0))
        assert app.wait(timeout=5) == 0, "Tray Quit did not stop the app"
        print("Wayland tray raise (no token, ignored token, and cross-workspace) passed.")


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=Path("target/debug/bluebubbles-native"))
    args = parser.parse_args()
    directory = Path(tempfile.mkdtemp(prefix="bluebubbles-wayland-tray."))
    test(args.binary.resolve(), directory)
