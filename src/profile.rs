use std::sync::OnceLock;
use std::time::{Duration, Instant};

use gtk::glib;

pub struct Span(Option<(&'static str, Instant)>);

fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("BLUEBUBBLES_NATIVE_PROFILE").is_some())
}

pub fn span(name: &'static str) -> Span {
    Span(enabled().then(|| (name, Instant::now())))
}

impl Drop for Span {
    fn drop(&mut self) {
        if let Some((name, start)) = self.0 {
            eprintln!(
                "profile {name}: {:.2} ms",
                start.elapsed().as_secs_f64() * 1000.0
            );
        }
    }
}

pub fn watch_main_loop() {
    if !enabled() {
        return;
    }
    let mut last_tick = Instant::now();
    glib::timeout_add_local(Duration::from_millis(16), move || {
        let now = Instant::now();
        let elapsed = now.duration_since(last_tick);
        if elapsed >= Duration::from_millis(32) {
            eprintln!(
                "profile main-loop gap: {:.2} ms",
                elapsed.as_secs_f64() * 1000.0
            );
        }
        last_tick = now;
        glib::ControlFlow::Continue
    });
}

pub fn sample_frames(widget: &impl gtk::prelude::IsA<gtk::Widget>, name: &'static str) {
    use gtk::prelude::*;
    if !enabled() {
        return;
    }
    let Some(clock) = widget.frame_clock() else {
        return;
    };
    let timing = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let observed = timing.clone();
    let id = clock.connect_after_paint(move |clock| {
        observed.borrow_mut().push(clock.frame_time());
    });
    let clock = clock.downgrade();
    glib::timeout_add_local_once(Duration::from_millis(300), move || {
        if let Some(clock) = clock.upgrade() {
            clock.disconnect(id);
        }
        let timing = timing.borrow();
        let longest = timing
            .windows(2)
            .map(|pair| pair[1] - pair[0])
            .max()
            .unwrap_or(0);
        eprintln!(
            "profile {name}: {} frames, max {:.2} ms",
            timing.len(),
            longest as f64 / 1000.0
        );
    });
}
