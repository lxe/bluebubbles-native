use std::f64::consts::TAU;

use adw::prelude::*;
use gtk::{cairo::Context, gdk};

#[derive(Clone, Copy)]
pub enum Icon {
    Settings,
    Pin,
    JumpLatest,
    Refresh,
    Attach,
    Send,
    Remove,
    Minus,
    Plus,
    Minimize,
    Image,
    Video,
    Audio,
    Play,
    Download,
    Error,
    Person,
    Group,
    Server,
    Archive,
    Desktop,
    Appearance,
}

impl Icon {
    pub fn widget(self) -> gtk::DrawingArea {
        self.sized_widget(17)
    }

    pub fn sized_widget(self, canvas: i32) -> gtk::DrawingArea {
        let icon = gtk::DrawingArea::builder()
            .content_width(canvas)
            .content_height(canvas)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .can_target(false)
            .build();
        icon.add_css_class("bb-icon");
        let max_size = f64::from(canvas);
        icon.set_draw_func(move |area, cr, width, height| {
            let size = f64::from(width.min(height)).min(max_size);
            cr.translate(
                (f64::from(width) - size) / 2.0,
                (f64::from(height) - size) / 2.0,
            );
            self.paint(cr, size, area.color());
        });
        icon
    }

    pub fn button(self, label: &str) -> gtk::Button {
        let button = gtk::Button::builder()
            .child(&self.widget())
            .tooltip_text(label)
            .valign(gtk::Align::Center)
            .build();
        button.update_property(&[gtk::accessible::Property::Label(label)]);
        button
    }

    pub fn paint(self, cr: &Context, size: f64, color: gdk::RGBA) {
        cr.set_source_rgba(
            color.red().into(),
            color.green().into(),
            color.blue().into(),
            color.alpha().into(),
        );
        cr.scale(size / 20.0, size / 20.0);
        cr.set_line_width(1.65);
        cr.set_line_cap(gtk::cairo::LineCap::Round);
        cr.set_line_join(gtk::cairo::LineJoin::Round);
        self.draw(cr);
        let _ = cr.stroke();
    }

    fn draw(self, cr: &Context) {
        match self {
            Self::Pin => {
                lines(cr, &[(5.8, 4.3), (14.2, 4.3)]);
                lines(
                    cr,
                    &[
                        (7.1, 4.3),
                        (7.1, 8.6),
                        (5.0, 10.8),
                        (5.0, 11.6),
                        (15.0, 11.6),
                        (15.0, 10.8),
                        (12.9, 8.6),
                        (12.9, 4.3),
                    ],
                );
                lines(cr, &[(10.0, 11.6), (10.0, 16.4)]);
            }
            Self::Settings => {
                cr.arc(10.0, 10.0, 5.3, 0.0, TAU);
                cr.new_sub_path();
                cr.arc(10.0, 10.0, 1.8, 0.0, TAU);
                for step in 0..8 {
                    let angle = TAU / 8.0 * f64::from(step);
                    cr.move_to(10.0 + angle.cos() * 5.4, 10.0 + angle.sin() * 5.4);
                    cr.line_to(10.0 + angle.cos() * 7.3, 10.0 + angle.sin() * 7.3);
                }
            }
            Self::JumpLatest => {
                lines(cr, &[(10.0, 3.5), (10.0, 15.8)]);
                lines(cr, &[(4.6, 10.4), (10.0, 15.8), (15.4, 10.4)]);
            }
            Self::Refresh => {
                cr.arc(10.0, 10.0, 6.0, 0.3, TAU - 0.57);
                lines(cr, &[(15.1, 3.5), (15.1, 7.2), (11.5, 7.2)]);
            }
            Self::Attach => {
                cr.move_to(13.8, 6.1);
                cr.line_to(7.7, 12.2);
                cr.curve_to(5.9, 14.0, 8.3, 16.4, 10.1, 14.6);
                cr.line_to(15.4, 9.3);
                cr.curve_to(19.1, 5.7, 14.1, 1.4, 11.0, 4.7);
                cr.line_to(5.1, 10.5);
                cr.curve_to(0.9, 14.7, 6.8, 19.4, 10.3, 15.8);
            }
            Self::Send => {
                lines(
                    cr,
                    &[
                        (3.3, 9.5),
                        (16.5, 3.6),
                        (12.2, 16.3),
                        (9.2, 11.3),
                        (3.3, 9.5),
                    ],
                );
                lines(cr, &[(9.2, 11.3), (16.5, 3.6)]);
            }
            Self::Remove => {
                lines(cr, &[(5.8, 5.8), (14.2, 14.2)]);
                lines(cr, &[(14.2, 5.8), (5.8, 14.2)]);
            }
            Self::Minus => {
                lines(cr, &[(4.8, 10.0), (15.2, 10.0)]);
            }
            Self::Plus => {
                lines(cr, &[(10.0, 4.8), (10.0, 15.2)]);
                lines(cr, &[(4.8, 10.0), (15.2, 10.0)]);
            }
            Self::Minimize => {
                lines(cr, &[(5.2, 12.2), (14.8, 12.2)]);
            }
            Self::Image => {
                box_outline(cr);
                cr.new_sub_path();
                cr.arc(7.0, 7.4, 1.15, 0.0, TAU);
                lines(
                    cr,
                    &[
                        (3.6, 14.6),
                        (8.0, 10.7),
                        (10.4, 12.8),
                        (13.7, 9.7),
                        (16.4, 12.3),
                    ],
                );
            }
            Self::Video => {
                box_outline(cr);
                lines(cr, &[(8.0, 7.1), (13.0, 10.0), (8.0, 12.9), (8.0, 7.1)]);
            }
            Self::Audio => {
                for (x, half) in [
                    (4.0, 1.9),
                    (7.0, 3.5),
                    (10.0, 5.6),
                    (13.0, 3.8),
                    (16.0, 2.1),
                ] {
                    lines(cr, &[(x, 10.0 - half), (x, 10.0 + half)]);
                }
            }
            Self::Play => {
                lines(cr, &[(6.0, 4.3), (15.1, 10.0), (6.0, 15.7), (6.0, 4.3)]);
            }
            Self::Download => {
                lines(cr, &[(10.0, 3.3), (10.0, 12.6)]);
                lines(cr, &[(6.5, 9.3), (10.0, 12.8), (13.5, 9.3)]);
                lines(cr, &[(4.1, 12.3), (4.1, 16.2), (15.9, 16.2), (15.9, 12.3)]);
            }
            Self::Person => {
                cr.arc(10.0, 6.4, 3.0, 0.0, TAU);
                shoulders(cr, 10.0, 5.8, 11.5, 16.5);
            }
            Self::Group => {
                for center in [6.2, 13.8] {
                    cr.new_sub_path();
                    cr.arc(center, 7.0, 2.2, 0.0, TAU);
                    shoulders(cr, center, 3.35, 11.8, 15.8);
                }
            }
            Self::Error => {
                cr.arc(10.0, 10.0, 6.8, 0.0, TAU);
                lines(cr, &[(10.0, 6.6), (10.0, 10.5)]);
                lines(cr, &[(10.0, 13.3), (10.0, 13.4)]);
            }
            Self::Server => {
                cr.rectangle(3.6, 4.2, 12.8, 4.8);
                cr.rectangle(3.6, 11.0, 12.8, 4.8);
                lines(cr, &[(6.4, 6.6), (6.5, 6.6)]);
                lines(cr, &[(6.4, 13.4), (6.5, 13.4)]);
                lines(cr, &[(9.4, 6.6), (13.6, 6.6)]);
                lines(cr, &[(9.4, 13.4), (13.6, 13.4)]);
            }
            Self::Archive => {
                cr.rectangle(3.4, 4.2, 13.2, 3.2);
                lines(cr, &[(4.6, 7.4), (4.6, 15.6), (15.4, 15.6), (15.4, 7.4)]);
                lines(cr, &[(8.1, 10.6), (11.9, 10.6)]);
            }
            Self::Desktop => {
                cr.rectangle(3.2, 4.2, 13.6, 9.2);
                lines(cr, &[(10.0, 13.4), (10.0, 16.0)]);
                lines(cr, &[(6.8, 16.0), (13.2, 16.0)]);
            }
            Self::Appearance => {
                cr.arc(10.0, 10.0, 6.4, 0.0, TAU);
                cr.new_sub_path();
                cr.arc(7.6, 8.2, 1.0, 0.0, TAU);
                cr.new_sub_path();
                cr.arc(12.4, 8.2, 1.0, 0.0, TAU);
                cr.new_sub_path();
                cr.arc(10.0, 12.4, 1.0, 0.0, TAU);
            }
        }
    }
}

fn box_outline(cr: &Context) {
    cr.rectangle(3.4, 4.3, 13.2, 11.4);
}

fn lines(cr: &Context, points: &[(f64, f64)]) {
    if let Some((start, rest)) = points.split_first() {
        cr.move_to(start.0, start.1);
        for &(x, y) in rest {
            cr.line_to(x, y);
        }
    }
}

fn shoulders(cr: &Context, x: f64, half_width: f64, top: f64, bottom: f64) {
    cr.move_to(x - half_width, bottom);
    let control = bottom + (top - bottom) * (4.0 / 3.0);
    cr.curve_to(
        x - half_width,
        control,
        x + half_width,
        control,
        x + half_width,
        bottom,
    );
}
