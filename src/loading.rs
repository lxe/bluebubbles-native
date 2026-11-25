use std::f64::consts::TAU;

use adw::prelude::*;
use gtk::glib;

pub fn animations_enabled() -> bool {
    gtk::Settings::default().is_none_or(|settings| settings.is_gtk_enable_animations())
}

pub fn orbit(size: i32) -> gtk::DrawingArea {
    let drawing = gtk::DrawingArea::builder()
        .content_width(size)
        .content_height(size)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .build();
    drawing.add_css_class("sync-orbit");
    drawing.set_draw_func(|area, context, width, height| {
        let color = area.color();
        let size = f64::from(width.min(height));
        let center_x = f64::from(width) / 2.0;
        let center_y = f64::from(height) / 2.0;
        let phase = if animations_enabled() {
            (glib::monotonic_time() % 1_450_000) as f64 / 1_450_000.0 * TAU
        } else {
            0.0
        };
        for index in 0..3 {
            let angle = phase + f64::from(index) * TAU / 3.0;
            let depth = (angle.sin() + 1.0) / 2.0;
            let x = center_x + angle.cos() * size * 0.29;
            let y = center_y + angle.sin() * size * 0.18;
            context.set_source_rgba(
                color.red().into(),
                color.green().into(),
                color.blue().into(),
                0.48 + depth * 0.52,
            );
            context.arc(x, y, size * (0.07 + depth * 0.035), 0.0, TAU);
            let _ = context.fill();
        }
    });
    drawing.add_tick_callback(|area, _| {
        if area.is_mapped() && animations_enabled() {
            area.queue_draw();
        }
        glib::ControlFlow::Continue
    });
    drawing
}

pub fn state(label: &str) -> gtk::Box {
    let container = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(9)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .build();
    let label = gtk::Label::new(Some(label));
    label.add_css_class("dim-label");
    container.append(&orbit(36));
    container.append(&label);
    container
}
