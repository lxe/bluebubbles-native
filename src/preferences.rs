use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, gio, glib};

use crate::api::Credentials;
use crate::icons::Icon;
use crate::organization::ArchivedRow;
use crate::settings::{self, TrayOptions};
use crate::theme::{self, Colorway};

const PREFERENCES_WIDTH: i32 = 740;
const PREFERENCES_HEIGHT: i32 = 560;
const SIDEBAR_WIDTH: i32 = 200;

#[derive(Clone)]
pub struct Preferences {
    dialog: adw::Dialog,
    toasts: adw::ToastOverlay,
    nav: gtk::ListBox,
    server_tab: gtk::ListBoxRow,
    server: adw::EntryRow,
    password: adw::PasswordEntryRow,
    remember: gtk::Switch,
    connect: gtk::Button,
    forget: gtk::Button,
    status: adw::ActionRow,
    status_icon: gtk::DrawingArea,
    progress: gtk::DrawingArea,
    archive_rows: gio::ListStore,
    archive_list: gtk::ListBox,
    desktop: Option<DesktopSettings>,
}

#[derive(Clone)]
struct DesktopSettings {
    minimize: adw::SwitchRow,
    close: adw::SwitchRow,
    notifications: adw::SwitchRow,
}

impl Preferences {
    pub fn new(
        server: &str,
        saved: bool,
        status: &str,
        error: bool,
        tray: Option<TrayOptions>,
        theme: Rc<theme::Controller>,
    ) -> Self {
        let dialog = adw::Dialog::builder()
            .title("Preferences")
            .presentation_mode(adw::DialogPresentationMode::Floating)
            .content_width(PREFERENCES_WIDTH)
            .content_height(PREFERENCES_HEIGHT)
            .build();
        dialog.add_css_class("bb-preferences");
        theme::add_text_size_shortcuts(&dialog);

        let nav = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::Single)
            .vexpand(true)
            .build();
        nav.add_css_class("preferences-nav");

        let stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .transition_duration(110)
            .vexpand(true)
            .hexpand(true)
            .build();

        let toasts = adw::ToastOverlay::new();

        let (
            server_page,
            server_entry,
            password,
            remember,
            connect,
            forget,
            connection_status,
            status_icon,
            progress,
        ) = build_server_page(server, saved, status, error);
        stack.add_named(&scroll_page(&server_page), Some("server"));

        let (archive_page, archive_rows, archive_list) = build_archive_page();
        stack.add_named(&archive_page, Some("archive"));

        let desktop = tray.map(|options| {
            let (page, controls) = build_desktop_page(options);
            stack.add_named(&scroll_page(&page), Some("desktop"));
            controls
        });

        let appearance_page = build_appearance_page(&toasts, theme);
        stack.add_named(&scroll_page(&appearance_page), Some("appearance"));

        let server_tab = nav_row("server", "Server", Icon::Server);
        let archive_tab = nav_row("archive", "Archived Chats", Icon::Archive);
        nav.append(&server_tab);
        nav.append(&archive_tab);
        if desktop.is_some() {
            nav.append(&nav_row("desktop", "Desktop", Icon::Desktop));
        }
        nav.append(&nav_row("appearance", "Appearance", Icon::Appearance));

        let sidebar_header = adw::HeaderBar::builder()
            .show_start_title_buttons(false)
            .show_end_title_buttons(false)
            .centering_policy(adw::CenteringPolicy::Strict)
            .title_widget(&adw::WindowTitle::new("Preferences", ""))
            .build();
        let sidebar_divider = gtk::Separator::new(gtk::Orientation::Horizontal);
        sidebar_divider.add_css_class("preferences-header-divider");
        let sidebar = gtk::Box::new(gtk::Orientation::Vertical, 0);
        sidebar.set_width_request(SIDEBAR_WIDTH);
        sidebar.add_css_class("preferences-sidebar");
        sidebar.append(&sidebar_header);
        sidebar.append(&sidebar_divider);
        sidebar.append(&nav);

        let pane_title = adw::WindowTitle::new("Server", "");
        let content_header = adw::HeaderBar::builder()
            .show_start_title_buttons(false)
            .show_end_title_buttons(true)
            .centering_policy(adw::CenteringPolicy::Strict)
            .title_widget(&pane_title)
            .build();
        let content_divider = gtk::Separator::new(gtk::Orientation::Horizontal);
        content_divider.add_css_class("preferences-header-divider");
        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.set_hexpand(true);
        content.add_css_class("preferences-content");
        content.append(&content_header);
        content.append(&content_divider);
        content.append(&stack);

        let header_height = gtk::SizeGroup::new(gtk::SizeGroupMode::Vertical);
        header_height.add_widget(&sidebar_header);
        header_height.add_widget(&content_header);

        let divider = gtk::Separator::new(gtk::Orientation::Vertical);
        divider.add_css_class("preferences-divider");

        let split = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        split.add_css_class("preferences-split");
        split.append(&sidebar);
        split.append(&divider);
        split.append(&content);

        nav.connect_row_selected({
            let stack = stack.downgrade();
            let pane_title = pane_title.downgrade();
            move |_, row| {
                let Some(row) = row else { return };
                let name = row.widget_name();
                if name == "archive" {
                    let _profile = crate::profile::span("archive-open-ui");
                }
                if let Some(stack) = stack.upgrade() {
                    stack.set_visible_child_name(&name);
                }
                if let Some(pane_title) = pane_title.upgrade() {
                    pane_title.set_title(tab_title(&name));
                }
            }
        });
        nav.select_row(Some(&server_tab));

        toasts.set_child(Some(&split));
        dialog.set_child(Some(&toasts));
        dialog.set_default_widget(Some(&connect));

        Self {
            dialog,
            toasts,
            nav,
            server_tab,
            server: server_entry,
            password,
            remember,
            connect,
            forget,
            status: connection_status,
            status_icon,
            progress,
            archive_rows,
            archive_list,
            desktop,
        }
    }

    pub fn wire_tray(&self, on_change: impl Fn(TrayOptions) -> bool + 'static) {
        let Some(DesktopSettings {
            minimize, close, ..
        }) = &self.desktop
        else {
            return;
        };
        let saved = std::rc::Rc::new(std::cell::Cell::new(TrayOptions {
            minimize: minimize.is_active(),
            close: close.is_active(),
        }));
        let on_change = std::rc::Rc::new(on_change);
        for switch in [minimize, close] {
            let (minimize, close) = (minimize.downgrade(), close.downgrade());
            let saved = saved.clone();
            let on_change = on_change.clone();
            let toasts = self.toasts.downgrade();
            switch.connect_active_notify(move |_| {
                let (Some(minimize), Some(close)) = (minimize.upgrade(), close.upgrade()) else {
                    return;
                };
                let options = TrayOptions {
                    minimize: minimize.is_active(),
                    close: close.is_active(),
                };
                if options == saved.get() {
                    return;
                }
                if on_change(options) {
                    saved.set(options);
                } else {
                    minimize.set_active(saved.get().minimize);
                    close.set_active(saved.get().close);
                    if let Some(toasts) = toasts.upgrade() {
                        toasts.add_toast(theme::error_toast("Could not save tray preferences."));
                    }
                }
            });
        }
    }

    pub fn wire_notifications(&self, on_change: impl Fn(bool) -> bool + 'static) {
        let Some(desktop) = &self.desktop else { return };
        let saved = std::cell::Cell::new(desktop.notifications.is_active());
        let toasts = self.toasts.downgrade();
        desktop.notifications.connect_active_notify(move |row| {
            let enabled = row.is_active();
            if enabled != saved.get() {
                if on_change(enabled) {
                    saved.set(enabled);
                } else {
                    row.set_active(saved.get());
                    if let Some(toasts) = toasts.upgrade() {
                        toasts.add_toast(theme::error_toast(
                            "Could not save notification preferences.",
                        ));
                    }
                }
            }
        });
    }

    pub fn wire(
        &self,
        on_connect: impl Fn(Credentials, bool) + 'static,
        on_forget: impl Fn() + 'static,
        on_restore: impl Fn(&str) + 'static,
    ) {
        let server = self.server.downgrade();
        let password = self.password.downgrade();
        let remember = self.remember.downgrade();
        let status = self.status.downgrade();
        let status_icon = self.status_icon.downgrade();
        let toasts = self.toasts.downgrade();
        self.connect.connect_clicked(move |_| {
            let (Some(server), Some(password), Some(remember), Some(status)) = (
                server.upgrade(),
                password.upgrade(),
                remember.upgrade(),
                status.upgrade(),
            ) else {
                return;
            };
            match Credentials::new(&server.text(), password.text().to_string()) {
                Ok(credentials) => on_connect(credentials, remember.is_active()),
                Err(error) => {
                    if let Some(icon) = status_icon.upgrade() {
                        apply_status(&status, &icon, &error, true);
                    }
                    if let Some(toasts) = toasts.upgrade() {
                        toasts.add_toast(theme::error_toast(&error));
                    }
                }
            }
        });
        self.forget.connect_clicked(move |_| on_forget());
        let on_restore = std::rc::Rc::new(on_restore);
        self.archive_list
            .bind_model(Some(&self.archive_rows), move |item| {
                let chat = item
                    .downcast_ref::<glib::BoxedAnyObject>()
                    .unwrap()
                    .borrow::<ArchivedRow>();
                let row = adw::ActionRow::builder()
                    .title(&chat.name)
                    .use_markup(false)
                    .build();
                row.set_title_lines(2);
                let restore = gtk::Button::with_label("Restore");
                restore.set_valign(gtk::Align::Center);
                restore.update_property(&[gtk::accessible::Property::Label(&format!(
                    "Restore {}",
                    chat.name
                ))]);
                let guid = chat.guid.clone();
                let on_restore = on_restore.clone();
                restore.connect_clicked(move |_| on_restore(&guid));
                row.add_suffix(&restore);
                row.upcast()
            });

        self.server.connect_entry_activated({
            let password = self.password.downgrade();
            move |_| {
                if let Some(password) = password.upgrade() {
                    password.grab_focus();
                }
            }
        });
        self.password.connect_entry_activated({
            let connect = self.connect.downgrade();
            move |_| {
                if let Some(connect) = connect.upgrade() {
                    connect.emit_clicked();
                }
            }
        });
    }

    pub fn set_archive(&self, archive: Vec<ArchivedRow>) {
        let _profile = crate::profile::span("archive-update-ui");
        crate::rows::replace_changed(&self.archive_rows, archive);
    }

    pub fn show_status(&self, status: &str, error: bool) {
        apply_status(&self.status, &self.status_icon, status, error);
        self.progress.set_visible(status == "Connecting…");
    }

    pub fn toast(&self, toast: adw::Toast) {
        self.toasts.add_toast(toast);
    }

    pub fn set_busy(&self, busy: bool, can_forget: bool) {
        self.server.set_editable(!busy);
        self.password.set_editable(!busy);
        self.remember.set_sensitive(!busy);
        self.connect.set_sensitive(!busy);
        self.forget.set_sensitive(!busy && can_forget);
    }

    pub fn clear_password(&self) {
        self.password.set_text("");
    }

    pub fn clear_connection(&self) {
        self.server.set_text("");
        self.clear_password();
        self.server.grab_focus();
    }

    pub fn present(&self, parent: &adw::ApplicationWindow, focus_connection: bool) {
        if focus_connection {
            self.nav.select_row(Some(&self.server_tab));
            if self.server.text().trim().is_empty() {
                self.dialog.set_focus(Some(&self.server));
            } else {
                self.dialog.set_focus(Some(&self.password));
            }
        }
        self.dialog.present(Some(parent));
        if focus_connection {
            if self.server.text().trim().is_empty() {
                self.server.grab_focus();
            } else {
                self.password.grab_focus();
            }
        }
    }

    pub fn close_handle(&self, handler: impl Fn() + 'static) {
        self.dialog.connect_closed(move |_| handler());
    }
}

fn nav_row(id: &str, title: &str, icon: Icon) -> gtk::ListBoxRow {
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    content.add_css_class("preferences-tab");
    content.append(&icon.widget());
    let label = gtk::Label::builder().label(title).xalign(0.0).build();
    content.append(&label);
    let row = gtk::ListBoxRow::builder().child(&content).build();
    row.set_widget_name(id);
    row.update_property(&[gtk::accessible::Property::Label(title)]);
    row
}

fn tab_title(id: &str) -> &'static str {
    match id {
        "server" => "Server",
        "archive" => "Archived Chats",
        "desktop" => "Desktop",
        "appearance" => "Appearance",
        _ => "Preferences",
    }
}

fn scroll_page(child: &impl IsA<gtk::Widget>) -> gtk::ScrolledWindow {
    gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::External)
        .child(child)
        .build()
}

fn page_box() -> gtk::Box {
    gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(20)
        .margin_top(20)
        .margin_bottom(20)
        .margin_start(22)
        .margin_end(22)
        .build()
}

#[allow(clippy::type_complexity)]
fn build_server_page(
    server: &str,
    saved: bool,
    status: &str,
    error: bool,
) -> (
    gtk::Box,
    adw::EntryRow,
    adw::PasswordEntryRow,
    gtk::Switch,
    gtk::Button,
    gtk::Button,
    adw::ActionRow,
    gtk::DrawingArea,
    gtk::DrawingArea,
) {
    let page = page_box();
    let group = adw::PreferencesGroup::builder()
        .title("BlueBubbles Server")
        .build();
    let server_entry = adw::EntryRow::builder()
        .title("Server URL")
        .input_purpose(gtk::InputPurpose::Url)
        .build();
    server_entry.set_text(server);
    let password = adw::PasswordEntryRow::builder()
        .title("Server password")
        .build();

    let remember = gtk::Switch::builder()
        .active(true)
        .valign(gtk::Align::Center)
        .build();
    let remember_row = adw::ActionRow::builder()
        .title("Remember connection")
        .subtitle("Store the password in your desktop keyring")
        .build();
    remember.update_property(&[gtk::accessible::Property::Label("Remember connection")]);
    remember_row.add_suffix(&remember);
    remember_row.set_activatable_widget(Some(&remember));

    let connection_status = adw::ActionRow::builder()
        .title("Status")
        .subtitle(status)
        .build();
    connection_status.set_subtitle_lines(0);
    let status_icon = Icon::Error.widget();
    status_icon.set_visible(false);
    connection_status.add_suffix(&status_icon);
    let progress = crate::loading::orbit(24);
    progress.set_visible(status == "Connecting…");
    connection_status.add_suffix(&progress);
    apply_status(&connection_status, &status_icon, status, error);

    group.add(&server_entry);
    group.add(&password);
    group.add(&remember_row);
    group.add(&connection_status);
    page.append(&group);

    let buttons = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .halign(gtk::Align::End)
        .build();
    let forget = gtk::Button::with_label("Forget Connection");
    forget.set_sensitive(saved);
    let connect = gtk::Button::with_label("Connect");
    connect.add_css_class("suggested-action");
    buttons.append(&forget);
    buttons.append(&connect);
    page.append(&buttons);

    (
        page,
        server_entry,
        password,
        remember,
        connect,
        forget,
        connection_status,
        status_icon,
        progress,
    )
}

fn build_archive_page() -> (gtk::Stack, gio::ListStore, gtk::ListBox) {
    let archive_rows = gio::ListStore::new::<glib::BoxedAnyObject>();
    let archive_list = gtk::ListBox::new();
    archive_list.set_selection_mode(gtk::SelectionMode::None);
    archive_list.add_css_class("boxed-list");
    let empty = adw::StatusPage::builder()
        .title("No archived chats")
        .description("Archive a conversation from its sidebar menu.")
        .build();
    empty.add_css_class("compact");
    let stack = gtk::Stack::new();
    stack.add_named(&empty, Some("empty"));
    let scroll = scroll_page(&archive_list);
    scroll.set_margin_top(20);
    scroll.set_margin_bottom(20);
    scroll.set_margin_start(22);
    scroll.set_margin_end(22);
    stack.add_named(&scroll, Some("list"));
    archive_rows.connect_items_changed({
        let stack = stack.downgrade();
        move |rows, _, _, _| {
            if let Some(stack) = stack.upgrade() {
                stack.set_visible_child_name(if rows.n_items() == 0 { "empty" } else { "list" });
            }
        }
    });
    (stack, archive_rows, archive_list)
}

fn build_desktop_page(options: TrayOptions) -> (gtk::Box, DesktopSettings) {
    let minimize = adw::SwitchRow::builder()
        .title("Minimize to Tray")
        .subtitle("Hide the window when you minimize it")
        .active(options.minimize)
        .build();
    let close = adw::SwitchRow::builder()
        .title("Close to Tray")
        .subtitle("Keep BlueBubbles running when you close the window")
        .active(options.close)
        .build();
    let tray_group = adw::PreferencesGroup::builder()
        .title("System Tray")
        .description("Use the tray icon to reopen or quit BlueBubbles.")
        .build();
    tray_group.add(&minimize);
    tray_group.add(&close);

    let notifications = adw::SwitchRow::builder()
        .title("Message Notifications")
        .subtitle("Show desktop notifications for new messages")
        .active(settings::notifications_enabled())
        .build();
    let notifications_group = adw::PreferencesGroup::builder()
        .title("Notifications")
        .build();
    notifications_group.add(&notifications);

    let page = page_box();
    page.append(&tray_group);
    page.append(&notifications_group);
    (
        page,
        DesktopSettings {
            minimize,
            close,
            notifications,
        },
    )
}

fn build_appearance_page(toasts: &adw::ToastOverlay, theme: Rc<theme::Controller>) -> gtk::Box {
    let options = page_box();
    options.add_css_class("appearance-page");
    let text = adw::PreferencesGroup::new();
    text.add(&text_size_row(theme.text_size_action()));
    options.append(&text);
    let group = adw::PreferencesGroup::builder().title("Colorway").build();
    options.append(&group);
    let actions = gio::SimpleActionGroup::new();
    let select = gio::SimpleAction::new_stateful(
        "theme",
        Some(glib::VariantTy::STRING),
        &theme.selection().key().to_variant(),
    );
    let toasts = toasts.downgrade();
    select.connect_activate(move |action, value| {
        let Some(choice) = value
            .and_then(glib::Variant::str)
            .and_then(Colorway::from_key)
        else {
            return;
        };
        if choice == theme.selection() {
            return;
        }
        if settings::save_colorway(choice).is_ok() {
            theme.select(choice);
            action.set_state(&choice.key().to_variant());
        } else if let Some(toasts) = toasts.upgrade() {
            toasts.add_toast(adw::Toast::new("Could not save the colorway"));
        }
    });
    actions.add_action(&select);
    options.insert_action_group("appearance", Some(&actions));
    for choice in Colorway::ALL {
        let row = adw::ActionRow::builder().title(choice.label()).build();
        let check = gtk::CheckButton::builder()
            .valign(gtk::Align::Center)
            .action_name("appearance.theme")
            .action_target(&choice.key().to_variant())
            .build();
        check.update_property(&[gtk::accessible::Property::Label(choice.label())]);
        row.add_suffix(&colorway_swatch(choice));
        row.add_suffix(&check);
        row.set_activatable_widget(Some(&check));
        group.add(&row);
    }
    options
}

fn text_size_row(size: &gio::SimpleAction) -> adw::ActionRow {
    let row = adw::ActionRow::builder().title("Text size").build();
    let controls = gtk::Box::builder().valign(gtk::Align::Center).build();
    controls.add_css_class("linked");
    controls.add_css_class("text-size-control");

    let smaller = gtk::Button::builder()
        .child(&Icon::Minus.sized_widget(15))
        .action_name("win.decrease-text-size")
        .tooltip_text("Decrease text size (Ctrl+−)")
        .build();
    smaller.add_css_class("text-size-step");
    smaller.update_property(&[gtk::accessible::Property::Label("Decrease text size")]);
    let larger = gtk::Button::builder()
        .child(&Icon::Plus.sized_widget(15))
        .action_name("win.increase-text-size")
        .tooltip_text("Increase text size (Ctrl++)")
        .build();
    larger.add_css_class("text-size-step");
    larger.update_property(&[gtk::accessible::Property::Label("Increase text size")]);
    let value = gtk::Label::builder().halign(gtk::Align::Center).build();
    let reset = gtk::Button::builder()
        .child(&value)
        .action_name("win.reset-text-size")
        .tooltip_text("Reset text size (Ctrl+0)")
        .build();
    reset.add_css_class("text-size-value");
    size.bind_property("state", &value, "label")
        .transform_to(|_, state: glib::Variant| state.get::<i32>().map(|size| format!("{size}%")))
        .sync_create()
        .build();
    controls.append(&smaller);
    controls.append(&reset);
    controls.append(&larger);
    row.add_suffix(&controls);
    row
}

fn colorway_swatch(choice: Colorway) -> gtk::DrawingArea {
    let swatch = gtk::DrawingArea::builder()
        .content_width(60)
        .content_height(16)
        .valign(gtk::Align::Center)
        .can_target(false)
        .build();
    let colors = choice
        .swatches()
        .map(|color| gdk::RGBA::parse(color).expect("Colorway color"));
    swatch.set_draw_func(move |area, cr, _, _| {
        let edge = area.color();
        for (index, color) in colors.iter().enumerate() {
            cr.arc(
                7.0 + 15.0 * index as f64,
                8.0,
                5.5,
                0.0,
                std::f64::consts::TAU,
            );
            cr.set_source_rgb(
                color.red().into(),
                color.green().into(),
                color.blue().into(),
            );
            let _ = cr.fill_preserve();
            cr.set_source_rgba(
                edge.red().into(),
                edge.green().into(),
                edge.blue().into(),
                0.20,
            );
            cr.set_line_width(1.0);
            let _ = cr.stroke();
        }
    });
    swatch
}

fn apply_status(row: &adw::ActionRow, icon: &gtk::DrawingArea, subtitle: &str, error: bool) {
    row.set_title(if error {
        "Connection problem"
    } else {
        "Status"
    });
    row.set_subtitle(subtitle);
    icon.set_visible(error);
    if error {
        row.add_css_class("connection-error");
    } else {
        row.remove_css_class("connection-error");
    }
}
