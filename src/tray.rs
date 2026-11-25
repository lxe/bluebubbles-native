use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use ksni::{Icon, MenuItem, TrayMethods, menu::StandardItem};

use crate::settings::TrayOptions;

enum Command {
    Open(Option<String>),
    Quit,
}

struct Item(async_channel::Sender<Command>);

impl Item {
    fn open(&self) {
        let _ = self
            .0
            .try_send(Command::Open(ksni::current_xdg_activation_token()));
    }
}

impl ksni::Tray for Item {
    fn id(&self) -> String {
        "bluebubbles-native".into()
    }

    fn title(&self) -> String {
        "BlueBubbles Native".into()
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        [16, 32].into_iter().filter_map(icon).collect()
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        self.open();
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        vec![
            StandardItem {
                label: "Open BlueBubbles".into(),
                activate: Box::new(|item: &mut Self| item.open()),
                ..StandardItem::default()
            }
            .into(),
            StandardItem {
                label: "Quit".into(),
                activate: Box::new(|item: &mut Self| {
                    let _ = item.0.try_send(Command::Quit);
                }),
                ..StandardItem::default()
            }
            .into(),
        ]
    }
}

pub struct Controller {
    window: glib::WeakRef<adw::ApplicationWindow>,
    options: Cell<TrayOptions>,
    watcher: RefCell<Option<gio::WatcherId>>,
    host_found: Cell<bool>,
    registering: Cell<bool>,
    revision: Cell<u64>,
    handle: RefCell<Option<ksni::Handle<Item>>>,
    hidden: Cell<bool>,
    controls: WindowControls,
}

pub struct WindowControls {
    pub sidebar: adw::HeaderBar,
    pub thread: adw::HeaderBar,
    pub wide_minimize: gtk::Button,
    pub narrow_minimize: gtk::Button,
    pub layout: gtk::Stack,
}

impl WindowControls {
    fn update(&self, enabled: bool) {
        let layout = self.thread.settings().gtk_decoration_layout();
        let override_layout = enabled
            .then(|| strip_minimize(layout.as_deref().unwrap_or(":close")))
            .flatten();
        self.sidebar
            .set_decoration_layout(override_layout.as_deref());
        self.thread
            .set_decoration_layout(override_layout.as_deref());
        self.wide_minimize.set_visible(enabled);
        self.narrow_minimize
            .set_visible(enabled && self.layout.visible_child_name().as_deref() == Some("narrow"));
    }
}

fn strip_minimize(layout: &str) -> Option<String> {
    let (start, end) = layout.split_once(':').unwrap_or(("", layout));
    let filter = |side: &str| {
        side.split(',')
            .filter(|item| !item.is_empty() && *item != "minimize")
            .collect::<Vec<_>>()
            .join(",")
    };
    let had_minimize = start
        .split(',')
        .chain(end.split(','))
        .any(|i| i == "minimize");
    had_minimize.then(|| format!("{}:{}", filter(start), filter(end)))
}

impl Controller {
    pub fn new(
        window: &adw::ApplicationWindow,
        options: TrayOptions,
        controls: WindowControls,
    ) -> Rc<Self> {
        let tray = Rc::new(Self {
            window: window.downgrade(),
            options: Cell::new(options),
            watcher: RefCell::new(None),
            host_found: Cell::new(false),
            registering: Cell::new(false),
            revision: Cell::new(0),
            handle: RefCell::new(None),
            hidden: Cell::new(false),
            controls,
        });
        tray.controls.update(options.minimize);
        tray.controls.layout.connect_visible_child_name_notify({
            let tray = Rc::downgrade(&tray);
            move |_| {
                if let Some(tray) = tray.upgrade() {
                    tray.controls.update(tray.options.get().minimize);
                }
            }
        });
        let (appeared, vanished) = (Rc::downgrade(&tray), Rc::downgrade(&tray));
        tray.watcher.replace(Some(gio::bus_watch_name(
            gio::BusType::Session,
            "org.kde.StatusNotifierWatcher",
            gio::BusNameWatcherFlags::NONE,
            move |_, _, _| {
                if let Some(tray) = appeared.upgrade() {
                    tray.host_found.set(true);
                    tray.register();
                }
            },
            move |_, _| {
                if let Some(tray) = vanished.upgrade() {
                    tray.host_found.set(false);
                    tray.unregister();
                }
            },
        )));
        window.connect_show({
            let tray = Rc::downgrade(&tray);
            move |_| {
                if let Some(tray) = tray.upgrade() {
                    tray.hidden.set(false);
                }
            }
        });
        window.connect_realize({
            let tray = Rc::downgrade(&tray);
            move |window| {
                if let Some(surface) = window.surface().and_downcast::<gdk::Toplevel>() {
                    let tray = tray.clone();
                    let minimized = Cell::new(false);
                    surface.connect_state_notify(move |surface| {
                        let state = surface.state().contains(gdk::ToplevelState::MINIMIZED);
                        if !minimized.replace(state)
                            && state
                            && let Some(tray) = tray.upgrade()
                            && tray.options.get().minimize
                        {
                            tray.hide();
                        }
                    });
                }
            }
        });
        tray
    }

    pub fn options(&self) -> TrayOptions {
        self.options.get()
    }

    pub fn set_options(self: &Rc<Self>, options: TrayOptions) {
        self.options.set(options);
        self.controls.update(options.minimize);
        if options.enabled() {
            self.register();
        } else {
            self.unregister();
        }
    }

    pub fn minimize(&self) {
        if self.options.get().minimize && self.hide() {
            return;
        }
        if let Some(window) = self.window.upgrade() {
            window.minimize();
        }
    }

    pub fn close_to_tray(&self) -> bool {
        self.options.get().close && self.hide()
    }

    pub fn open(&self, token: Option<&str>) {
        if let Some(window) = self.window.upgrade() {
            self.hidden.set(false);
            if let Some(token) = token {
                window.set_startup_id(token);
            }
            window.present();
            window.unminimize();
        }
    }

    fn hide(&self) -> bool {
        if !self
            .handle
            .borrow()
            .as_ref()
            .is_some_and(|handle| !handle.is_closed())
        {
            return false;
        }
        let Some(window) = self.window.upgrade() else {
            return false;
        };
        self.hidden.set(true);
        window.set_visible(false);
        true
    }

    fn register(self: &Rc<Self>) {
        if !self.options.get().enabled()
            || !self.host_found.get()
            || self.handle.borrow().is_some()
            || self.registering.replace(true)
        {
            return;
        }
        let revision = self.revision.get();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let (send, receive) = async_channel::unbounded();
            let result = Item(send).spawn().await;
            {
                let Some(tray) = weak
                    .upgrade()
                    .filter(|tray| tray.revision.get() == revision)
                else {
                    if let Ok(handle) = result {
                        handle.shutdown().await;
                    }
                    return;
                };
                tray.registering.set(false);
                let Ok(handle) = result else { return };
                tray.handle.replace(Some(handle));
            }
            while let Ok(command) = receive.recv().await {
                let Some(tray) = weak
                    .upgrade()
                    .filter(|tray| tray.revision.get() == revision)
                else {
                    break;
                };
                match command {
                    Command::Open(token) => tray.open(token.as_deref()),
                    Command::Quit => {
                        if let Some(window) = tray.window.upgrade() {
                            let _ =
                                gtk::prelude::WidgetExt::activate_action(&window, "win.quit", None);
                        }
                    }
                }
            }
        });
    }

    fn unregister(&self) {
        self.revision.set(self.revision.get().wrapping_add(1));
        self.registering.set(false);
        if let Some(handle) = self.handle.take() {
            drop(handle.shutdown());
        }
        if self.hidden.get() {
            self.open(None);
        }
    }
}

impl Drop for Controller {
    fn drop(&mut self) {
        if let Some(watcher) = self.watcher.take() {
            gio::bus_unwatch_name(watcher);
        }
        if let Some(handle) = self.handle.take() {
            drop(handle.shutdown());
        }
    }
}

fn icon(size: i32) -> Option<Icon> {
    use gtk::cairo::{Context, Format, ImageSurface};

    let mut surface = ImageSurface::create(Format::ARgb32, size, size).ok()?;
    let cr = Context::new(&surface).ok()?;
    cr.scale(f64::from(size) / 32.0, f64::from(size) / 32.0);
    cr.set_source_rgb(0.29, 0.55, 0.77);
    cr.move_to(12.0, 4.0);
    cr.line_to(22.0, 4.0);
    cr.curve_to(27.0, 4.0, 30.0, 7.0, 30.0, 12.0);
    cr.curve_to(30.0, 17.0, 27.0, 20.0, 22.0, 20.0);
    cr.line_to(12.0, 20.0);
    cr.fill().ok()?;
    cr.set_source_rgb(0.57, 0.8, 1.0);
    cr.move_to(10.0, 9.0);
    cr.line_to(20.0, 9.0);
    cr.curve_to(24.0, 9.0, 27.0, 12.5, 27.0, 17.0);
    cr.curve_to(27.0, 21.5, 24.0, 25.0, 20.0, 25.0);
    cr.line_to(11.0, 25.0);
    cr.line_to(5.5, 28.0);
    cr.line_to(6.0, 24.0);
    cr.curve_to(3.5, 23.0, 2.0, 20.0, 2.0, 17.0);
    cr.curve_to(2.0, 12.5, 5.0, 9.0, 10.0, 9.0);
    cr.fill().ok()?;
    cr.set_source_rgb(0.095, 0.16, 0.23);
    for x in [8.5, 14.5, 20.5] {
        cr.new_sub_path();
        cr.arc(x, 17.0, 1.35, 0.0, std::f64::consts::TAU);
    }
    cr.fill().ok()?;
    drop(cr);

    // Cairo uses native-endian premultiplied pixels. SNI uses straight ARGB.
    let pixels = surface.data().ok()?;
    let data = pixels
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|pixel| {
            let [a, r, g, b] = u32::from_ne_bytes(*pixel).to_be_bytes();
            let straight = |value: u8| {
                if a == 0 {
                    0
                } else {
                    (u32::from(value) * 255 / u32::from(a)).min(255) as u8
                }
            };
            [a, straight(r), straight(g), straight(b)]
        })
        .collect();
    Some(Icon {
        width: size,
        height: size,
        data,
    })
}

#[cfg(test)]
mod tests {
    use super::strip_minimize;

    #[test]
    fn strip_minimize_removes_only_minimize_token() {
        assert_eq!(strip_minimize(":close"), None);
        assert_eq!(strip_minimize("appmenu:close"), None);
        assert_eq!(
            strip_minimize(":minimize,maximize,close"),
            Some(":maximize,close".into())
        );
        assert_eq!(
            strip_minimize("close,minimize:menu"),
            Some("close:menu".into())
        );
    }
}
