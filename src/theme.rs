use std::cell::Cell;
use std::fmt::Write;
use std::rc::Rc;

use adw::prelude::*;
use gtk::gio;

use crate::settings;

pub const DEFAULT_TEXT_SIZE: i32 = 100;
const MIN_TEXT_SIZE: i32 = 80;
const MAX_TEXT_SIZE: i32 = 160;
const TEXT_SIZE_STEP: i32 = 10;
const INCREASE_TEXT_SIZE: &str = "increase-text-size";
const DECREASE_TEXT_SIZE: &str = "decrease-text-size";
const RESET_TEXT_SIZE: &str = "reset-text-size";

pub fn error_toast(message: &str) -> adw::Toast {
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    content.add_css_class("error-toast");
    content.append(&crate::icons::Icon::Error.widget());
    let text = gtk::Label::builder()
        .label(message)
        .xalign(0.0)
        .wrap(true)
        .max_width_chars(54)
        .build();
    content.append(&text);
    adw::Toast::builder()
        .custom_title(&content)
        .timeout(8)
        .build()
}

pub fn add_text_size_shortcuts(widget: &impl IsA<gtk::Widget>) {
    let shortcuts = gtk::ShortcutController::new();
    shortcuts.set_propagation_phase(gtk::PropagationPhase::Capture);
    for (action, keys) in [
        (
            INCREASE_TEXT_SIZE,
            "<Control>plus|<Control>equal|<Control>KP_Add",
        ),
        (DECREASE_TEXT_SIZE, "<Control>minus|<Control>KP_Subtract"),
        (RESET_TEXT_SIZE, "<Control>0|<Control>KP_0"),
    ] {
        shortcuts.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string(keys),
            Some(gtk::NamedAction::new(&format!("win.{action}"))),
        ));
    }
    widget.add_controller(shortcuts);
}

const ROLES: &[&str] = &[
    "canvas",
    "sidebar",
    "header",
    "surface",
    "text",
    "muted",
    "border",
    "divider",
    "accent",
    "accent_text",
    "selected",
    "outgoing",
    "incoming",
    "error",
    "error_surface",
];
const LIGHT: &[&str] = &[
    "#f7f9fc", "#edf3f8", "#eef3f8", "#ffffff", "#172534", "#465c70", "#cbd8e3", "#d3dfe9",
    "#185f94", "#ffffff", "#cfe4f5", "#dceefa", "#e9f0f6", "#a32439", "#fae4e9",
];
const DARK: &[&str] = &[
    "#121b24", "#192633", "#1b2836", "#202f3e", "#f6f9fc", "#b7cadb", "#3b5062", "#293b4c",
    "#91cbff", "#102334", "#304e69", "#294a64", "#223443", "#ffacb8", "#492a34",
];
const GRAPHITE: &[&str] = &[
    "#0d0e11", "#14161a", "#14161a", "#1d2026", "#f0f1f4", "#aeb5c1", "#343944", "#2b3039",
    "#a3c9f5", "#111b2a", "#202b39", "#24374d", "#1b1e24", "#ffadb8", "#3e272f",
];
const CHAMPAGNE: &[&str] = &[
    "#100e0d", "#181513", "#181513", "#221d19", "#f4f0eb", "#bcb0a4", "#473b32", "#342c25",
    "#dbbf9c", "#261b11", "#302620", "#382c22", "#211d1a", "#ffb3a7", "#402824",
];
const PERIWINKLE: &[&str] = &[
    "#0d0e17", "#151621", "#151621", "#202232", "#f0f0f8", "#b1b4cd", "#40445b", "#2c3043",
    "#b2bcfa", "#161b38", "#272b40", "#2d3251", "#1b1d2a", "#ffadca", "#3e283b",
];
const NEON: &[&str] = &[
    "#08090b", "#101115", "#121317", "#191b20", "#f1f3f7", "#adb2be", "#343844", "#282b33",
    "#5aebe2", "#092220", "#182324", "#30234b", "#17191e", "#ff9cb8", "#3d2230",
];
const PORCELAIN: &[&str] = &[
    "#f5f7fb", "#eaf0f8", "#edf2f9", "#ffffff", "#1a2335", "#49576e", "#c3cedf", "#cfd9e8",
    "#264dad", "#ffffff", "#d9e4fa", "#dee8fb", "#ffffff", "#9f2440", "#f8e4ea",
];
const IVORY: &[&str] = &[
    "#f7f4ed", "#eee8dd", "#f1ece2", "#fffdf7", "#30251f", "#605147", "#cbbdad", "#ded3c3",
    "#8a4930", "#fff9f2", "#ecddce", "#f0dfd1", "#fffaf1", "#972e37", "#f5dedc",
];
const HIGH_CONTRAST: &[&str] = &[
    "@window_bg_color",
    "@sidebar_bg_color",
    "@headerbar_bg_color",
    "@card_bg_color",
    "@window_fg_color",
    "@window_fg_color",
    "@borders",
    "@borders",
    "@accent_bg_color",
    "@accent_fg_color",
    "mix(@window_bg_color, @accent_bg_color, 0.25)",
    "mix(@window_bg_color, @accent_bg_color, 0.25)",
    "@card_bg_color",
    "@error_color",
    "mix(@window_bg_color, @error_bg_color, 0.15)",
];

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Colorway {
    #[default]
    System,
    Graphite,
    Champagne,
    Periwinkle,
    Neon,
    Porcelain,
    Ivory,
}

impl Colorway {
    pub const ALL: [Self; 7] = [
        Self::System,
        Self::Graphite,
        Self::Champagne,
        Self::Periwinkle,
        Self::Neon,
        Self::Porcelain,
        Self::Ivory,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Graphite => "graphite",
            Self::Champagne => "champagne",
            Self::Periwinkle => "periwinkle",
            Self::Neon => "neon",
            Self::Porcelain => "porcelain",
            Self::Ivory => "ivory",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|choice| choice.key() == key)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::System => "Follow System",
            Self::Graphite => "Graphite and Ice Blue",
            Self::Champagne => "Warm Charcoal and Champagne",
            Self::Periwinkle => "Ink and Periwinkle",
            Self::Neon => "Black and Neon",
            Self::Porcelain => "Porcelain and Cobalt",
            Self::Ivory => "Ivory and Copper",
        }
    }

    fn scheme(self) -> adw::ColorScheme {
        match self {
            Self::System => adw::ColorScheme::Default,
            Self::Porcelain | Self::Ivory => adw::ColorScheme::ForceLight,
            _ => adw::ColorScheme::ForceDark,
        }
    }

    fn palette(self, dark: bool) -> &'static [&'static str] {
        match self {
            Self::System if dark => DARK,
            Self::System => LIGHT,
            Self::Graphite => GRAPHITE,
            Self::Champagne => CHAMPAGNE,
            Self::Periwinkle => PERIWINKLE,
            Self::Neon => NEON,
            Self::Porcelain => PORCELAIN,
            Self::Ivory => IVORY,
        }
    }

    pub fn swatches(self) -> [&'static str; 4] {
        if self == Self::System {
            return [LIGHT[0], LIGHT[8], DARK[0], DARK[8]];
        }
        let palette = self.palette(false);
        [palette[0], palette[3], palette[11], palette[8]]
    }
}

pub struct Controller {
    provider: gtk::CssProvider,
    text_provider: gtk::CssProvider,
    manager: adw::StyleManager,
    selection: Cell<Colorway>,
    increase_text: gio::SimpleAction,
    decrease_text: gio::SimpleAction,
    reset_text: gio::SimpleAction,
}

impl Controller {
    pub fn install(window: &adw::ApplicationWindow, toasts: &adw::ToastOverlay) -> Rc<Self> {
        let display = RootExt::display(window);
        let (selection, text_size) = settings::appearance();
        let text_size = text_size.clamp(MIN_TEXT_SIZE, MAX_TEXT_SIZE);
        let controller = Rc::new(Self {
            provider: gtk::CssProvider::new(),
            text_provider: gtk::CssProvider::new(),
            manager: adw::StyleManager::for_display(&display),
            selection: Cell::new(selection),
            increase_text: gio::SimpleAction::new(INCREASE_TEXT_SIZE, None),
            decrease_text: gio::SimpleAction::new(DECREASE_TEXT_SIZE, None),
            reset_text: gio::SimpleAction::new_stateful(
                RESET_TEXT_SIZE,
                None,
                &text_size.to_variant(),
            ),
        });
        controller.apply();
        controller.manager.connect_dark_notify({
            let weak = Rc::downgrade(&controller);
            move |_| {
                if let Some(controller) = weak.upgrade() {
                    controller.reload();
                }
            }
        });
        controller.manager.connect_high_contrast_notify({
            let weak = Rc::downgrade(&controller);
            move |_| {
                if let Some(controller) = weak.upgrade() {
                    controller.reload();
                }
            }
        });
        gtk::style_context_add_provider_for_display(
            &display,
            &controller.provider,
            gtk::STYLE_PROVIDER_PRIORITY_USER + 1,
        );
        gtk::style_context_add_provider_for_display(
            &display,
            &controller.text_provider,
            gtk::STYLE_PROVIDER_PRIORITY_USER + 1,
        );
        window.add_css_class("bb-window");
        controller.apply_text_size(text_size);
        for (action, step) in [
            (&controller.increase_text, TEXT_SIZE_STEP),
            (&controller.decrease_text, -TEXT_SIZE_STEP),
            (&controller.reset_text, 0),
        ] {
            let controller = Rc::downgrade(&controller);
            let toasts = toasts.downgrade();
            action.connect_activate(move |_, _| {
                let Some(controller) = controller.upgrade() else {
                    return;
                };
                let current = controller.text_size();
                let size = if step == 0 {
                    DEFAULT_TEXT_SIZE
                } else {
                    (current + step).clamp(MIN_TEXT_SIZE, MAX_TEXT_SIZE)
                };
                if size == current {
                    return;
                }
                if settings::save_text_size(size).is_ok() {
                    controller.apply_text_size(size);
                } else if let Some(toasts) = toasts.upgrade() {
                    toasts.add_toast(adw::Toast::new("Could not save the text size"));
                }
            });
            window.add_action(action);
        }
        add_text_size_shortcuts(window);
        controller
    }

    pub fn text_size_action(&self) -> &gio::SimpleAction {
        &self.reset_text
    }

    fn text_size(&self) -> i32 {
        self.reset_text
            .state()
            .and_then(|value| value.get())
            .unwrap_or(DEFAULT_TEXT_SIZE)
    }

    fn apply_text_size(&self, size: i32) {
        let css = if size == DEFAULT_TEXT_SIZE {
            String::new()
        } else {
            format!(".bb-window {{ font-size: {size}%; }}")
        };
        self.text_provider.load_from_string(&css);
        self.reset_text.set_state(&size.to_variant());
        self.increase_text.set_enabled(size < MAX_TEXT_SIZE);
        self.decrease_text.set_enabled(size > MIN_TEXT_SIZE);
    }

    pub fn selection(&self) -> Colorway {
        self.selection.get()
    }

    pub fn select(&self, selection: Colorway) {
        if self.selection.replace(selection) != selection {
            self.apply();
        }
    }

    fn apply(&self) {
        self.manager.set_color_scheme(self.selection().scheme());
        self.reload();
    }

    fn reload(&self) {
        let selection = self.selection();
        let high_contrast = self.manager.is_high_contrast();
        let dark = self.manager.is_dark();
        let (colors, timestamp) = if high_contrast {
            (HIGH_CONTRAST, "@window_fg_color")
        } else if dark {
            (selection.palette(true), "mix(@bb_muted, @bb_canvas, 0.24)")
        } else {
            (selection.palette(false), "mix(@bb_muted, @bb_canvas, 0.12)")
        };
        let link = match (high_contrast, selection) {
            (true, _) | (_, Colorway::System) => "@bb_text",
            (_, Colorway::Neon) => "#c6aaff",
            _ => "@bb_accent",
        };
        let mut css = String::new();
        for (name, color) in ROLES.iter().zip(colors) {
            writeln!(css, "@define-color bb_{name} {color};").unwrap();
        }
        writeln!(css, "@define-color bb_timestamp {timestamp};").unwrap();
        writeln!(css, "@define-color bb_link {link};").unwrap();
        css.push_str(include_str!("style.css"));
        self.provider.load_from_string(&css);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colorways_round_trip_and_match_css_roles() {
        for colorway in Colorway::ALL {
            assert_eq!(Colorway::from_key(colorway.key()), Some(colorway));
            assert_eq!(colorway.palette(false).len(), ROLES.len());
            assert_eq!(colorway.palette(true).len(), ROLES.len());
            for swatch in colorway.swatches() {
                assert!(
                    gtk::gdk::RGBA::parse(swatch).is_ok(),
                    "invalid swatch {swatch}"
                );
            }
        }
        assert_eq!(HIGH_CONTRAST.len(), ROLES.len());
        assert_eq!(Colorway::from_key("unknown"), None);
    }
}
