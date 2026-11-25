#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

mode=${1:---fast}

run_xvfb() {
    env -u WAYLAND_DISPLAY -u HYPRLAND_INSTANCE_SIGNATURE XDG_CURRENT_DESKTOP=GNOME \
        xvfb-run -a -s '-screen 0 1280x900x24' dbus-run-session -- "$@"
}

run_fast() {
    echo "==> Formatting"
    cargo fmt --all --check
    echo "==> Clippy"
    cargo clippy --locked --all-targets -- -D warnings
    echo "==> Unit tests"
    cargo test --locked
}

run_gui() {
    echo "==> Building debug binary"
    cargo build --locked --quiet
    echo "==> Colorways and text size"
    run_xvfb python3 tests/colorways.py
    echo "==> System tray"
    run_xvfb python3 tests/tray.py
    echo "==> Visual smoke"
    tests/visual-smoke.sh
    echo "==> Thread features"
    tests/thread-features.sh
    echo "==> Clipboard media paste"
    run_xvfb python3 tests/clipboard_paste.py
    echo "==> Realtime & notifications"
    run_xvfb python3 tests/realtime.py
    echo "==> Thread switching"
    run_xvfb python3 tests/thread_switch.py --large
}

case "$mode" in
    --fast)
        run_fast
        ;;
    --gui)
        run_gui
        ;;
    --all)
        run_fast
        run_gui
        echo "==> Wayland header seam"
        tests/visual-wayland-seam.sh
        ;;
    -h|--help)
        cat <<'EOF'
Usage: tests/run.sh [--fast | --gui | --all]

  --fast  Run rustfmt, clippy (-D warnings), and Rust unit tests (default)
  --gui   Run isolated Xvfb GUI integration suites against local fixtures
  --all   Run --fast, --gui, and the Wayland Sway header seam check
EOF
        ;;
    *)
        echo "Unknown option: $mode (use --fast, --gui, or --all)" >&2
        exit 2
        ;;
esac
