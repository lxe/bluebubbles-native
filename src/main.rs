mod api;
mod avatar;
mod bubble_lane;
mod cache;
mod context_menu;
mod icons;
mod layout;
mod links;
mod loading;
mod media;
mod model;
mod organization;
mod preferences;
mod profile;
mod reaction_stack;
mod realtime;
mod rows;
mod sample;
mod settings;
mod squircle;
mod theme;
mod thread;
mod timeline_view;
mod tray;
mod workspace;

use adw::prelude::*;
use gtk::glib;
use workspace::{Mode, Workspace};

fn main() -> glib::ExitCode {
    let args = std::env::args_os().collect::<Vec<_>>();
    if args.iter().any(|arg| arg == "--check-connection") {
        return match api::check_connection() {
            Ok(()) => glib::ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("{error}");
                glib::ExitCode::FAILURE
            }
        };
    }

    let (mode, id) = if args.iter().any(|arg| arg == "--patterns") {
        (Mode::Patterns, "co.lxe.BlueBubblesNative.Patterns")
    } else if std::env::var_os("BLUEBUBBLES_NATIVE_STRESS").is_some() {
        (Mode::Stress, "co.lxe.BlueBubblesNative.Stress")
    } else {
        (Mode::Live, "co.lxe.BlueBubblesNative")
    };
    let app = adw::Application::builder().application_id(id).build();
    app.connect_startup(|_| profile::watch_main_loop());
    app.connect_activate(move |app| {
        if let Some(window) = app
            .active_window()
            .or_else(|| app.windows().into_iter().next())
        {
            window.present();
            window.unminimize();
        } else {
            Workspace::open(app, mode);
        }
    });
    app.run_with_args(&["bluebubbles-native"])
}
