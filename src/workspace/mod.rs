mod chats;
mod composer;
mod media;
mod timeline;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use adw::prelude::*;
use gtk::{gdk, gio, glib};

use crate::api::{BlueBubbles, Credentials};
use crate::avatar::Avatar;
use crate::cache::Cache;
use crate::icons::Icon;
use crate::layout::{Layout, SIDEBAR_INSET, SIDEBAR_ROW_GAP, shortcut, thread_clamp};
use crate::model::{Attachment, Conversation, TapbackKind};
use crate::organization::{ChatAction, ChatOrganization, SidebarRow};
use crate::preferences::Preferences;
use crate::thread::Quote;
use crate::timeline_view::TimelineView;
use crate::{loading, media as media_ui, realtime, rows, sample, settings, theme, tray};

use composer::SendJob;
use timeline::{Fetch, LoadedThread};

#[derive(Clone, Copy, PartialEq)]
pub enum Mode {
    Live,
    Patterns,
    Stress,
}

#[derive(Default, Clone)]
struct Draft {
    text: String,
    files: Vec<PathBuf>,
    reply: Option<ReplyDraft>,
}

#[derive(Clone)]
struct ReplyDraft {
    target: crate::model::MessagePointer,
    quote: Quote,
}

type RecoveredChats = Vec<(Conversation, i64)>;

type AvatarTargets = Vec<glib::WeakRef<Avatar>>;

enum GroupAvatar {
    Waiting(String, AvatarTargets),
    Loading(AvatarTargets),
    Ready(gdk::Texture),
    RetryAfter(Instant),
}

#[derive(Default)]
struct State {
    server: Option<BlueBubbles>,
    server_address: String,
    status: String,
    status_is_error: bool,
    active: Option<Conversation>,
    chats: Vec<Conversation>,
    organization: ChatOrganization,
    drafts: HashMap<String, Draft>,
    files: Vec<PathBuf>,
    reply: Option<ReplyDraft>,
    threads: HashMap<String, LoadedThread>,
    previews: VecDeque<(String, gdk::Texture)>,
    viewer_images: VecDeque<(String, i32, gdk::Texture)>,
    group_avatars: HashMap<String, GroupAvatar>,
    loading_previews: HashMap<String, Vec<media_ui::InlinePreview>>,
    restoring_connection: bool,
    connecting: bool,
    refreshing: bool,
    show_refresh: bool,
    last_sidebar_check: Option<Instant>,
    live: bool,
    was_live: bool,
    seen_notifications: VecDeque<String>,
    live_during_refresh: HashSet<String>,
    session: u64,
    outbox: HashMap<String, VecDeque<SendJob>>,
}

const ACTIVE_SYNC_INTERVAL: Duration = Duration::from_secs(15);
const MEMORY_THREAD_LIMIT: usize = 64;
const TIMELINE_VIEW_LIMIT: usize = 8;

pub struct Workspace {
    mode: Mode,
    theme: Rc<theme::Controller>,
    window: glib::WeakRef<adw::ApplicationWindow>,
    layout: Layout,
    overlays: adw::ToastOverlay,
    conversation_store: gio::ListStore,
    sidebar_revision: Rc<std::cell::Cell<u64>>,
    active_highlight: gtk::EntryBuffer,
    timeline_pages: gtk::Stack,
    timeline_view: RefCell<Rc<TimelineView>>,
    timeline_cache: RefCell<VecDeque<(String, Rc<TimelineView>)>>,
    saved_scroll: RefCell<HashMap<String, f64>>,
    counts: Rc<rows::Counts>,
    attachment_handler: media_ui::Handler,
    load_more: gtk::Button,
    jump_latest: gtk::Button,
    deferred_previews: RefCell<Vec<(Attachment, media_ui::InlinePreview)>>,
    preview_check: std::cell::Cell<bool>,
    thread_stack: gtk::Stack,
    loading_indicator: RefCell<Option<glib::SourceId>>,
    thread_status: adw::StatusPage,
    connect_button: gtk::Button,
    sidebar_connect: gtk::Button,
    thread_title: adw::WindowTitle,
    thread_avatar: Avatar,
    refresh: gtk::Button,
    refresh_content: gtk::Stack,
    compose: gtk::Box,
    editor: gtk::TextView,
    editor_scroll: gtk::ScrolledWindow,
    editor_entry: gtk::Overlay,
    composer_grid: gtk::Grid,
    editor_placeholder: gtk::Label,
    send: gtk::Button,
    attach: gtk::Button,
    pending: gtk::Box,
    prefs: RefCell<Option<Preferences>>,
    tray: Option<Rc<tray::Controller>>,
    realtime: RefCell<Option<realtime::Listener>>,
    notifications: std::cell::Cell<bool>,
    notification_start: i64,
    state: RefCell<State>,
}

struct SidebarWidgets {
    toolbar: adw::ToolbarView,
    header: adw::HeaderBar,
    search: gtk::SearchEntry,
    filtered: gtk::FilterListModel,
    chat_list: gtk::ListView,
    preferences_button: gtk::Button,
    sidebar_connect: gtk::Button,
    refresh: gtk::Button,
    refresh_content: gtk::Stack,
    sidebar_minimize: gtk::Button,
    sidebar_close: gtk::Button,
}

struct ComposerWidgets {
    compose: gtk::Box,
    compose_clamp: adw::Clamp,
    editor: gtk::TextView,
    scroll: gtk::ScrolledWindow,
    entry: gtk::Overlay,
    grid: gtk::Grid,
    placeholder: gtk::Label,
    send: gtk::Button,
    attach: gtk::Button,
    pending: gtk::Box,
}

fn build_sidebar(
    window: &adw::ApplicationWindow,
    mode: Mode,
    conversation_store: &gio::ListStore,
    counts: &Rc<rows::Counts>,
    active_highlight: &gtk::EntryBuffer,
    group_avatar_handler: &rows::GroupAvatarHandler,
) -> SidebarWidgets {
    let search = gtk::SearchEntry::builder()
        .placeholder_text("Search conversations")
        .search_delay(100)
        .hexpand(true)
        .valign(gtk::Align::Center)
        .build();
    let query = Rc::new(RefCell::new(Vec::<String>::new()));
    let filter = gtk::CustomFilter::new({
        let query = query.clone();
        move |item| {
            item.downcast_ref::<glib::BoxedAnyObject>()
                .and_then(|item| item.try_borrow::<SidebarRow>().ok())
                .is_some_and(|row| {
                    query
                        .borrow()
                        .iter()
                        .all(|word| row.chat.search_text.contains(word))
                })
        }
    });
    let filtered = gtk::FilterListModel::new(Some(conversation_store.clone()), None::<gtk::Filter>);
    let chat_list = gtk::ListView::new(
        Some(gtk::NoSelection::new(Some(filtered.clone()))),
        Some(rows::conversations(
            counts,
            active_highlight,
            group_avatar_handler,
        )),
    );
    chat_list.set_single_click_activate(true);
    chat_list.add_css_class("conversations");

    let sidebar_list = gtk::Stack::new();
    sidebar_list.set_vexpand(true);
    sidebar_list.set_margin_top(SIDEBAR_INSET - SIDEBAR_ROW_GAP);
    let sidebar_scroll = scroller(&chat_list);
    sidebar_scroll.add_css_class("sidebar-scroll");
    sidebar_list.add_named(&sidebar_scroll, Some("list"));
    let no_results = adw::StatusPage::builder().title("No conversations").build();
    no_results.add_css_class("compact");
    sidebar_list.add_named(&no_results, Some("empty"));

    let update_sidebar = Rc::new({
        let sidebar_list = sidebar_list.downgrade();
        let no_results = no_results.downgrade();
        let filtered = filtered.downgrade();
        let query = query.clone();
        move || {
            if let (Some(sidebar_list), Some(no_results), Some(filtered)) = (
                sidebar_list.upgrade(),
                no_results.upgrade(),
                filtered.upgrade(),
            ) {
                no_results.set_title(if query.borrow().is_empty() {
                    "No conversations"
                } else {
                    "No matching conversations"
                });
                sidebar_list.set_visible_child_name(if filtered.n_items() == 0 {
                    "empty"
                } else {
                    "list"
                });
            }
        }
    });
    filtered.connect_items_changed({
        let update_sidebar = update_sidebar.clone();
        move |_, _, _, _| update_sidebar()
    });
    search.connect_search_changed({
        let update_sidebar = update_sidebar.clone();
        let filtered = filtered.clone();
        move |search| {
            let words = search
                .text()
                .split_whitespace()
                .map(str::to_lowercase)
                .collect::<Vec<_>>();
            let empty = words.is_empty();
            let was_empty = query.replace(words).is_empty();
            match (was_empty, empty) {
                (true, false) => filtered.set_filter(Some(&filter)),
                (false, true) => filtered.set_filter(gtk::Filter::NONE),
                (false, false) => filter.changed(gtk::FilterChange::Different),
                (true, true) => {}
            }
            update_sidebar();
        }
    });
    update_sidebar();

    let sidebar_body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    sidebar_body.add_css_class("bb-sidebar");
    sidebar_body.append(&sidebar_list);
    let sidebar_connect = gtk::Button::with_label("Open Preferences");
    sidebar_connect.set_halign(gtk::Align::Center);
    sidebar_connect.set_margin_bottom(24);
    sidebar_connect.set_visible(false);
    sidebar_body.append(&sidebar_connect);

    let preferences_button = Icon::Settings.button("Preferences");
    let refresh_content = gtk::Stack::new();
    refresh_content.add_named(&Icon::Refresh.widget(), Some("idle"));
    refresh_content.add_named(&loading::orbit(23), Some("syncing"));
    let refresh = gtk::Button::builder()
        .child(&refresh_content)
        .valign(gtk::Align::Center)
        .tooltip_text("Refresh conversations")
        .visible(mode == Mode::Live)
        .sensitive(false)
        .build();
    refresh.update_property(&[gtk::accessible::Property::Label("Refresh conversations")]);

    let sidebar_minimize = gtk::Button::builder()
        .child(&Icon::Minimize.widget())
        .tooltip_text("Minimize to tray")
        .action_name("window.minimize")
        .visible(false)
        .valign(gtk::Align::Center)
        .build();
    sidebar_minimize.update_property(&[gtk::accessible::Property::Label("Minimize window")]);

    let sidebar_close = Icon::Remove.button("Close window");
    sidebar_close.set_visible(false);
    sidebar_close.connect_clicked({
        let weak = window.downgrade();
        move |_| {
            if let Some(window) = weak.upgrade() {
                window.close();
            }
        }
    });

    let sidebar_controls = gtk::Box::new(gtk::Orientation::Horizontal, SIDEBAR_INSET);
    sidebar_controls.set_hexpand(true);
    // Account for the edge pixel painted by the window or pane divider.
    sidebar_controls.set_margin_start(SIDEBAR_INSET + 1);
    sidebar_controls.set_margin_end(SIDEBAR_INSET + 1);
    sidebar_controls.append(&preferences_button);
    sidebar_controls.append(&search);
    sidebar_controls.append(&refresh);
    sidebar_controls.append(&sidebar_minimize);
    sidebar_controls.append(&sidebar_close);

    let sidebar_height = gtk::SizeGroup::new(gtk::SizeGroupMode::Vertical);
    sidebar_height.add_widget(&search);
    for button in [
        &preferences_button,
        &refresh,
        &sidebar_minimize,
        &sidebar_close,
    ] {
        button.add_css_class("sidebar-header-action");
        sidebar_height.add_widget(button);
    }

    let (toolbar, header) = toolbar(&sidebar_controls, &sidebar_body);
    header.add_css_class("sidebar-header");
    SidebarWidgets {
        toolbar,
        header,
        search,
        filtered,
        chat_list,
        preferences_button,
        sidebar_connect,
        refresh,
        refresh_content,
        sidebar_minimize,
        sidebar_close,
    }
}

fn build_composer() -> ComposerWidgets {
    let editor = gtk::TextView::builder()
        .wrap_mode(gtk::WrapMode::WordChar)
        .accepts_tab(false)
        .top_margin(6)
        .bottom_margin(4)
        .left_margin(4)
        .right_margin(4)
        .build();
    editor.update_property(&[gtk::accessible::Property::Label("Message")]);
    let scroll = gtk::ScrolledWindow::builder()
        .hexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::External)
        .min_content_height(0)
        .propagate_natural_height(false)
        .height_request(28)
        .child(&editor)
        .build();
    scroll.add_css_class("message-entry");
    let placeholder = gtk::Label::builder()
        .label("Message")
        .halign(gtk::Align::Start)
        .valign(gtk::Align::Start)
        .can_target(false)
        .build();
    placeholder.add_css_class("compose-placeholder");
    let entry = gtk::Overlay::new();
    entry.set_hexpand(true);
    entry.set_child(Some(&scroll));
    entry.add_overlay(&placeholder);

    let attach = Icon::Attach.button("Attach files");
    attach.add_css_class("flat");
    attach.add_css_class("circular");
    attach.add_css_class("compose-control");
    attach.set_valign(gtk::Align::Center);
    attach.set_halign(gtk::Align::Start);

    let send = gtk::Button::builder()
        .tooltip_text("Send message (Enter)")
        .child(&Icon::Send.widget())
        .valign(gtk::Align::Center)
        .halign(gtk::Align::End)
        .build();
    send.update_property(&[gtk::accessible::Property::Label("Send message")]);
    send.add_css_class("flat");
    send.add_css_class("compose-control");
    send.add_css_class("send-control");

    let grid = gtk::Grid::builder()
        .column_spacing(5)
        .row_spacing(4)
        .build();
    grid.add_css_class("compose-surface");
    grid.attach(&attach, 0, 0, 1, 1);
    grid.attach(&entry, 1, 0, 1, 1);
    grid.attach(&send, 2, 0, 1, 1);

    let pending = gtk::Box::new(gtk::Orientation::Vertical, 4);
    pending.set_visible(false);
    let compose = gtk::Box::new(gtk::Orientation::Vertical, 8);
    compose.add_css_class("composer");
    compose.append(&pending);
    compose.append(&grid);
    compose.set_visible(false);
    compose.add_css_class("thread-content-inset");
    let compose_clamp = thread_clamp(&compose);

    ComposerWidgets {
        compose,
        compose_clamp,
        editor,
        scroll,
        entry,
        grid,
        placeholder,
        send,
        attach,
        pending,
    }
}

impl Workspace {
    pub fn open(app: &adw::Application, mode: Mode) {
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title(if mode == Mode::Patterns {
                "BlueBubbles Patterns"
            } else {
                "BlueBubbles Native"
            })
            .default_width(1024)
            .default_height(700)
            .width_request(360)
            .height_request(400)
            .build();

        let conversation_store = gio::ListStore::new::<glib::BoxedAnyObject>();
        let counts = Rc::new(rows::Counts::default());
        let group_avatar_handler = rows::GroupAvatarHandler::default();
        let active_highlight = gtk::EntryBuffer::new(Some(""));
        let sidebar = build_sidebar(
            &window,
            mode,
            &conversation_store,
            &counts,
            &active_highlight,
            &group_avatar_handler,
        );

        let attachment_handler: media_ui::Handler = Rc::new(RefCell::new(None));
        let timeline_view = Rc::new(TimelineView::new(&counts, &attachment_handler));
        let timeline_pages = gtk::Stack::builder()
            .hhomogeneous(false)
            .vhomogeneous(false)
            .build();
        timeline_pages.add_child(&timeline_view.scroll);
        let thread_stack = gtk::Stack::new();
        thread_stack.set_vexpand(true);
        let timeline_area = gtk::Overlay::new();
        timeline_area.set_child(Some(&timeline_pages));
        let load_more = gtk::Button::with_label("Load More Messages");
        load_more.add_css_class("load-more-messages");
        load_more.set_halign(gtk::Align::Center);
        load_more.set_valign(gtk::Align::Start);
        load_more.set_margin_top(8);
        load_more.set_visible(false);
        timeline_area.add_overlay(&load_more);
        let jump_latest = Icon::JumpLatest.button("Jump to latest messages");
        jump_latest.add_css_class("jump-latest");
        jump_latest.set_halign(gtk::Align::End);
        jump_latest.set_valign(gtk::Align::End);
        jump_latest.set_margin_end(17);
        jump_latest.set_margin_bottom(10);
        jump_latest.set_visible(false);
        timeline_area.add_overlay(&jump_latest);
        thread_stack.add_named(&timeline_area, Some("timeline"));
        let thread_status = adw::StatusPage::new();
        thread_status.add_css_class("compact");
        let connect_button = gtk::Button::with_label("Open Preferences");
        connect_button.add_css_class("suggested-action");
        connect_button.set_halign(gtk::Align::Center);
        thread_status.set_child(Some(&connect_button));
        thread_stack.add_named(&thread_status, Some("status"));
        thread_stack.add_named(&gtk::Box::default(), Some("waiting"));
        thread_stack.add_named(&loading::state("Loading messages…"), Some("loading"));
        thread_stack.set_visible_child_name("status");

        let composer = build_composer();
        let thread_body = gtk::Box::new(gtk::Orientation::Vertical, 0);
        thread_body.append(&thread_stack);
        thread_body.append(&composer.compose_clamp);
        let thread_title = adw::WindowTitle::new("BlueBubbles Native", "");
        let thread_avatar = Avatar::new(28);
        thread_avatar.set_visible(false);
        let title_group = gtk::Box::new(gtk::Orientation::Horizontal, 9);
        title_group.append(&thread_avatar);
        title_group.append(&thread_title);
        let (thread, thread_header) = toolbar(&title_group, &thread_body);
        let thread_minimize = gtk::Button::builder()
            .child(&Icon::Minimize.sized_widget(14))
            .tooltip_text("Minimize to tray")
            .action_name("window.minimize")
            .visible(false)
            .valign(gtk::Align::Center)
            .build();
        thread_minimize.add_css_class("flat");
        thread_minimize.add_css_class("circular");
        thread_minimize.add_css_class("window-minimize-control");
        thread_minimize.update_property(&[gtk::accessible::Property::Label("Minimize window")]);
        thread_header.pack_end(&thread_minimize);

        let layout = Layout::new(
            &window,
            &sidebar.toolbar,
            &sidebar.header,
            &thread,
            &thread_header,
            mode == Mode::Live,
        );
        let overlays = adw::ToastOverlay::new();
        overlays.set_child(Some(&layout.widget));
        window.set_content(Some(&overlays));
        let theme = theme::Controller::install(&window, &overlays);

        let server_address = if mode == Mode::Live {
            settings::saved_server()
        } else {
            String::new()
        };
        let state = State {
            restoring_connection: !server_address.is_empty(),
            organization: if server_address.is_empty() {
                ChatOrganization::default()
            } else {
                settings::chat_organization(&server_address)
            },
            server_address,
            status: "Not connected".into(),
            ..State::default()
        };
        let tray = (mode == Mode::Live).then(|| {
            tray::Controller::new(
                &window,
                settings::tray_options(),
                tray::WindowControls {
                    sidebar: sidebar.header.clone(),
                    thread: thread_header.clone(),
                    wide_minimize: thread_minimize,
                    narrow_minimize: sidebar.sidebar_minimize,
                    layout: layout.widget.clone(),
                },
            )
        });
        let this = Rc::new(Self {
            mode,
            theme,
            window: window.downgrade(),
            layout,
            overlays,
            conversation_store,
            sidebar_revision: Rc::new(std::cell::Cell::new(0)),
            active_highlight,
            timeline_pages,
            timeline_view: RefCell::new(timeline_view),
            timeline_cache: RefCell::new(VecDeque::new()),
            saved_scroll: RefCell::new(HashMap::new()),
            counts: counts.clone(),
            attachment_handler: attachment_handler.clone(),
            load_more,
            jump_latest,
            deferred_previews: RefCell::new(Vec::new()),
            preview_check: std::cell::Cell::new(false),
            thread_stack,
            loading_indicator: RefCell::new(None),
            thread_status,
            connect_button,
            sidebar_connect: sidebar.sidebar_connect,
            thread_title,
            thread_avatar,
            refresh: sidebar.refresh,
            refresh_content: sidebar.refresh_content,
            compose: composer.compose,
            editor: composer.editor,
            editor_scroll: composer.scroll,
            editor_entry: composer.entry,
            composer_grid: composer.grid,
            editor_placeholder: composer.placeholder,
            send: composer.send,
            attach: composer.attach,
            pending: composer.pending,
            prefs: RefCell::new(None),
            tray,
            realtime: RefCell::new(None),
            notifications: std::cell::Cell::new(
                mode == Mode::Live && settings::notifications_enabled(),
            ),
            notification_start: glib::real_time() / 1000,
            state: RefCell::new(state),
        });

        this.wire_window_actions(app, &window, &sidebar.header, &thread_header);
        this.wire_sidebar_and_messages(
            &window,
            &sidebar.chat_list,
            &sidebar.filtered,
            &sidebar.preferences_button,
            &sidebar.sidebar_close,
            &sidebar.search,
            &group_avatar_handler,
            &attachment_handler,
        );
        this.wire_composer();
        this.start_session(app, &window, &counts);
    }

    fn wire_window_actions(
        self: &Rc<Self>,
        app: &adw::Application,
        window: &adw::ApplicationWindow,
        sidebar_header: &adw::HeaderBar,
        thread_header: &adw::HeaderBar,
    ) {
        if let Some(tray) = &self.tray {
            let controls = gio::SimpleActionGroup::new();
            let minimize = gio::SimpleAction::new("minimize", None);
            let tray = Rc::downgrade(tray);
            minimize.connect_activate({
                let tray = tray.clone();
                move |_, _| {
                    if let Some(tray) = tray.upgrade() {
                        tray.minimize();
                    }
                }
            });
            controls.add_action(&minimize);
            sidebar_header.insert_action_group("window", Some(&controls));
            thread_header.insert_action_group("window", Some(&controls));
            shortcut(window, "<Control>m", move || {
                if let Some(tray) = tray.upgrade() {
                    tray.minimize();
                }
            });

            let quit = gio::SimpleAction::new("quit", None);
            let weak = Rc::downgrade(self);
            quit.connect_activate(move |_, _| {
                if let Some(app) = weak.upgrade() {
                    app.request_quit();
                }
            });
            window.add_action(&quit);
            app.set_accels_for_action("win.quit", &["<Control>q"]);
        }
        if self.mode == Mode::Live {
            let open = gio::SimpleAction::new("open-conversation", Some(glib::VariantTy::STRING));
            let weak = Rc::downgrade(self);
            open.connect_activate(move |_, parameter| {
                if let (Some(app), Some(guid)) =
                    (weak.upgrade(), parameter.and_then(glib::Variant::str))
                {
                    app.open_notification(guid);
                }
            });
            app.add_action(&open);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn wire_sidebar_and_messages(
        self: &Rc<Self>,
        window: &adw::ApplicationWindow,
        chat_list: &gtk::ListView,
        filtered: &gtk::FilterListModel,
        preferences_button: &gtk::Button,
        sidebar_close: &gtk::Button,
        search: &gtk::SearchEntry,
        group_avatar_handler: &rows::GroupAvatarHandler,
        attachment_handler: &media_ui::Handler,
    ) {
        group_avatar_handler.replace(Some(Box::new({
            let weak = Rc::downgrade(self);
            move |chat, photo, target| {
                if let Some(app) = weak.upgrade() {
                    app.request_group_avatar(chat, photo, target);
                }
            }
        })));
        attachment_handler.replace(Some(Box::new({
            let weak = Rc::downgrade(self);
            move |attachment| {
                if let Some(app) = weak.upgrade() {
                    app.media_request(attachment);
                }
            }
        })));

        preferences_button.connect_clicked({
            let weak = Rc::downgrade(self);
            move |_| {
                if let Some(app) = weak.upgrade() {
                    app.preferences(false);
                }
            }
        });
        for button in [&self.connect_button, &self.sidebar_connect] {
            button.connect_clicked({
                let weak = Rc::downgrade(self);
                move |_| {
                    if let Some(app) = weak.upgrade() {
                        app.preferences(true);
                    }
                }
            });
        }
        self.layout.widget.connect_visible_child_notify({
            let weak = Rc::downgrade(self);
            let sidebar_close = sidebar_close.clone();
            move |layout| {
                sidebar_close.set_visible(layout.visible_child_name().as_deref() == Some("narrow"));
                if let Some(app) = weak.upgrade() {
                    app.update_connect_actions();
                    app.update_active_highlight();
                }
            }
        });
        self.layout.on_page_changed({
            let weak = Rc::downgrade(self);
            move || {
                if let Some(app) = weak.upgrade() {
                    app.update_active_highlight();
                }
            }
        });

        let actions = gio::SimpleActionGroup::new();
        for (name, action) in [
            ("pin", ChatAction::TogglePin),
            ("archive", ChatAction::Archive),
        ] {
            let handler = gio::SimpleAction::new(name, Some(glib::VariantTy::STRING));
            let weak = Rc::downgrade(self);
            handler.connect_activate(move |_, parameter| {
                if let (Some(app), Some(guid)) =
                    (weak.upgrade(), parameter.and_then(glib::Variant::str))
                {
                    app.organize_chat(action, guid);
                }
            });
            actions.add_action(&handler);
        }
        chat_list.insert_action_group("chat", Some(&actions));

        let message_actions = gio::SimpleActionGroup::new();
        for name in ["reply", "jump", "restore"] {
            let action = gio::SimpleAction::new(name, Some(glib::VariantTy::STRING));
            let weak = Rc::downgrade(self);
            action.connect_activate(move |_, value| {
                if let (Some(app), Some(guid)) =
                    (weak.upgrade(), value.and_then(glib::Variant::str))
                {
                    match name {
                        "reply" => app.reply_to(guid),
                        "restore" => app.restore_unsent(guid),
                        _ => {
                            if let Some(position) = app.message_position(guid) {
                                app.view().follow_latest.set(false);
                                app.scroll_to_message(position);
                            }
                        }
                    }
                }
            });
            message_actions.add_action(&action);
        }
        let copy = gio::SimpleAction::new("copy", Some(glib::VariantTy::STRING));
        let weak_window = window.downgrade();
        copy.connect_activate(move |_, value| {
            if let (Some(window), Some(text)) =
                (weak_window.upgrade(), value.and_then(glib::Variant::str))
            {
                gtk::prelude::WidgetExt::display(&window)
                    .clipboard()
                    .set_text(text);
            }
        });
        message_actions.add_action(&copy);
        let reaction_type = glib::VariantTy::new("(ss)").expect("reaction variant type");
        let react = gio::SimpleAction::new("react", Some(reaction_type));
        let weak = Rc::downgrade(self);
        react.connect_activate(move |_, value| {
            if let (Some(app), Some((guid, kind))) = (
                weak.upgrade(),
                value.and_then(glib::Variant::get::<(String, String)>),
            ) && let Some(kind) = TapbackKind::parse(&kind)
            {
                app.react_to(&guid, kind);
            }
        });
        message_actions.add_action(&react);
        self.timeline_pages
            .insert_action_group("message", Some(&message_actions));

        let context_key = gtk::EventControllerKey::new();
        context_key.set_propagation_phase(gtk::PropagationPhase::Capture);
        let list = chat_list.downgrade();
        context_key.connect_key_pressed(move |_, key, _, modifiers| {
            if crate::context_menu::keyboard_shortcut(key, modifiers)
                && list
                    .upgrade()
                    .is_some_and(|list| rows::open_focused_chat_menu(&list))
            {
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        chat_list.add_controller(context_key);
        chat_list.connect_activate({
            let weak = Rc::downgrade(self);
            let filtered = filtered.clone();
            move |_, index| {
                let Some(app) = weak.upgrade() else { return };
                if let Some(row) = filtered.item(index).and_downcast::<glib::BoxedAnyObject>() {
                    app.activate_chat(row.borrow::<SidebarRow>().chat.clone(), true);
                }
            }
        });

        self.load_more.connect_clicked({
            let weak = Rc::downgrade(self);
            move |_| {
                if let Some(app) = weak.upgrade() {
                    app.load_messages(Fetch::Older);
                }
            }
        });
        self.jump_latest.connect_clicked({
            let weak = Rc::downgrade(self);
            move |_| {
                if let Some(app) = weak.upgrade() {
                    let _profile = crate::profile::span("thread-jump-latest-ui");
                    app.scroll_latest();
                    app.update_load_more();
                }
            }
        });
        self.refresh.connect_clicked({
            let weak = Rc::downgrade(self);
            move |_| {
                if let Some(app) = weak.upgrade() {
                    app.refresh_chats(true);
                    app.load_messages(Fetch::CatchUp);
                }
            }
        });

        shortcut(window, "<Control>f", {
            let weak = Rc::downgrade(self);
            let search = search.clone();
            move || {
                if let Some(app) = weak.upgrade() {
                    app.layout.show_sidebar();
                    search.grab_focus();
                }
            }
        });
        shortcut(window, "<Control>comma", {
            let weak = Rc::downgrade(self);
            move || {
                if let Some(app) = weak.upgrade() {
                    app.preferences(false);
                }
            }
        });
    }

    fn wire_composer(self: &Rc<Self>) {
        self.attach.connect_clicked({
            let weak = Rc::downgrade(self);
            move |_| {
                if let Some(app) = weak.upgrade() {
                    app.pick_files();
                }
            }
        });
        self.send.connect_clicked({
            let weak = Rc::downgrade(self);
            move |_| {
                if let Some(app) = weak.upgrade() {
                    app.send_draft();
                }
            }
        });
        self.editor.buffer().connect_changed({
            let weak = Rc::downgrade(self);
            move |_| {
                if let Some(app) = weak.upgrade() {
                    app.update_composer();
                    app.resize_editor();
                }
            }
        });
        self.editor_scroll.hadjustment().connect_page_size_notify({
            let weak = Rc::downgrade(self);
            move |_| {
                if let Some(app) = weak.upgrade() {
                    app.resize_editor();
                }
            }
        });
        self.theme.text_size_action().connect_state_notify({
            let weak = Rc::downgrade(self);
            move |_| {
                let weak = weak.clone();
                glib::idle_add_local_once(move || {
                    if let Some(app) = weak.upgrade() {
                        app.resize_editor();
                    }
                });
            }
        });
        let enter = gtk::EventControllerKey::new();
        enter.connect_key_pressed({
            let weak = Rc::downgrade(self);
            move |_, key, _, modifiers| {
                let line_break = modifiers.intersects(
                    gdk::ModifierType::SHIFT_MASK
                        | gdk::ModifierType::CONTROL_MASK
                        | gdk::ModifierType::ALT_MASK,
                );
                if matches!(key, gdk::Key::Return | gdk::Key::KP_Enter)
                    && !line_break
                    && let Some(app) = weak.upgrade()
                {
                    app.send_draft();
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            }
        });
        self.editor.add_controller(enter);
        let drop = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
        drop.connect_drop({
            let weak = Rc::downgrade(self);
            move |_, value, _, _| {
                let (Some(app), Ok(files)) = (weak.upgrade(), value.get::<gdk::FileList>()) else {
                    return false;
                };
                let Some(guid) = app.active_guid() else {
                    return false;
                };
                app.add_files(
                    &guid,
                    files.files().into_iter().filter_map(|file| file.path()),
                );
                true
            }
        });
        self.compose.add_controller(drop);
    }

    fn start_session(
        self: &Rc<Self>,
        app: &adw::Application,
        window: &adw::ApplicationWindow,
        counts: &Rc<rows::Counts>,
    ) {
        if self.mode == Mode::Live {
            glib::timeout_add_seconds_local(5, {
                let weak = Rc::downgrade(self);
                move || {
                    let Some(app) = weak.upgrade() else {
                        return glib::ControlFlow::Break;
                    };
                    let (live, check_sidebar) = {
                        let state = app.state.borrow();
                        let interval = Duration::from_secs(if state.live { 120 } else { 15 });
                        (
                            state.live,
                            state
                                .last_sidebar_check
                                .is_none_or(|last| last.elapsed() >= interval),
                        )
                    };
                    if !live
                        && app
                            .window
                            .upgrade()
                            .is_some_and(|window| window.is_active())
                    {
                        app.load_messages(Fetch::Latest);
                    }
                    if check_sidebar {
                        app.refresh_chats(false);
                    }
                    glib::ControlFlow::Continue
                }
            });
        } else if self.mode == Mode::Stress {
            app.connect_shutdown({
                let counts = counts.clone();
                move |_| counts.report()
            });
        }

        window.connect_close_request({
            let this = self.clone();
            move |_| {
                if this.tray.as_ref().is_some_and(|tray| tray.close_to_tray()) {
                    return glib::Propagation::Stop;
                }
                if !this.state.borrow().outbox.is_empty() {
                    this.notify("A message is still sending.");
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            }
        });
        window.present();

        if self.mode == Mode::Live {
            self.show_disconnected();
            let saved_address = self.state.borrow().server_address.clone();
            if !saved_address.is_empty() {
                let cache = Cache::new(&saved_address);
                background(move || Ok(cache.conversations().conversations), {
                    let weak = Rc::downgrade(self);
                    move |result: Result<Vec<Conversation>, String>| {
                        if let (Some(app), Ok(rows)) = (weak.upgrade(), result)
                            && {
                                let state = app.state.borrow();
                                state.server_address == saved_address && state.chats.is_empty()
                            }
                            && !rows.is_empty()
                        {
                            app.replace_chats(rows);
                            if !app.restore_last_thread() {
                                app.status_page("Select a conversation", None);
                            }
                        }
                    }
                });
            }
            background(settings::load, {
                let weak = Rc::downgrade(self);
                move |result| {
                    let Some(app) = weak.upgrade() else {
                        return;
                    };
                    app.state.borrow_mut().restoring_connection = false;
                    if app.state.borrow().server.is_some() || app.state.borrow().connecting {
                        return;
                    }
                    match result {
                        Ok(Some(credentials)) => app.connect(credentials, None),
                        Err(error) => {
                            app.connection_error(&error);
                            app.update_empty_thread();
                        }
                        Ok(None) => app.update_empty_thread(),
                    }
                }
            });
        } else {
            let mut rows = sample::conversations();
            if self.mode == Mode::Patterns {
                rows.truncate(7);
            }
            self.replace_chats(rows);
            self.set_status("Offline sample data");
            self.status_page("Select a conversation", None);
        }
    }

    fn preferences(self: &Rc<Self>, focus_connection: bool) {
        let _profile = crate::profile::span("preferences-open-ui");
        let Some(window) = self.window.upgrade() else {
            return;
        };
        if let Some(prefs) = self.prefs.borrow().as_ref() {
            prefs.present(&window, focus_connection);
            return;
        }
        let state = self.state.borrow();
        let prefs = Preferences::new(
            &state.server_address,
            !settings::saved_server().is_empty(),
            &state.status,
            state.status_is_error,
            self.tray.as_ref().map(|tray| tray.options()),
            self.theme.clone(),
        );
        prefs.set_busy(
            state.connecting || self.mode == Mode::Stress,
            !settings::saved_server().is_empty(),
        );
        prefs.set_archive(state.organization.archive(&state.chats));
        drop(state);
        prefs.wire(
            {
                let weak = Rc::downgrade(self);
                move |credentials, remember| {
                    if let Some(app) = weak.upgrade() {
                        app.connect(credentials, Some(remember));
                    }
                }
            },
            {
                let weak = Rc::downgrade(self);
                move || {
                    if let Some(app) = weak.upgrade() {
                        app.forget();
                    }
                }
            },
            {
                let weak = Rc::downgrade(self);
                move |guid| {
                    if let Some(app) = weak.upgrade() {
                        app.organize_chat(ChatAction::Restore, guid);
                    }
                }
            },
        );
        if let Some(tray) = &self.tray {
            let tray = Rc::downgrade(tray);
            prefs.wire_tray(move |options| {
                let Some(tray) = tray.upgrade() else {
                    return false;
                };
                if settings::save_tray_options(options).is_err() {
                    return false;
                }
                tray.set_options(options);
                true
            });
        }
        prefs.wire_notifications({
            let weak = Rc::downgrade(self);
            move |enabled| {
                let Some(app) = weak.upgrade() else {
                    return false;
                };
                if settings::save_notifications(enabled).is_err() {
                    return false;
                }
                app.notifications.set(enabled);
                true
            }
        });
        prefs.close_handle({
            let weak = Rc::downgrade(self);
            move || {
                if let Some(app) = weak.upgrade() {
                    app.prefs.replace(None);
                }
            }
        });
        self.prefs.replace(Some(prefs.clone()));
        prefs.present(
            &window,
            focus_connection || self.state.borrow().server.is_none(),
        );
    }

    fn request_quit(&self) {
        if !self.state.borrow().outbox.is_empty() {
            if let Some(tray) = &self.tray {
                tray.open(None);
            }
            self.notify("A message is still sending.");
            return;
        }
        if let Some(app) = self
            .window
            .upgrade()
            .and_then(|window| window.application())
        {
            app.quit();
        }
    }

    fn connect(self: &Rc<Self>, credentials: Credentials, remember: Option<bool>) {
        if self.mode != Mode::Live
            || self.state.borrow().connecting
            || !self.state.borrow().outbox.is_empty()
        {
            return;
        }
        self.state.borrow_mut().connecting = true;
        self.set_status("Connecting…");
        self.update_connection_controls();
        self.update_composer();
        let server_address = credentials.server.to_string();
        background(
            move || {
                let mut server = BlueBubbles::new(credentials.clone())?;
                let version = server.info()?;
                let persistence_error = match remember {
                    Some(true) => settings::remember(&credentials).err(),
                    Some(false) => settings::forget().err(),
                    None => None,
                };
                Ok((server, version, persistence_error))
            },
            {
                let weak = Rc::downgrade(self);
                move |result: Result<(BlueBubbles, String, Option<String>), String>| {
                    let Some(app) = weak.upgrade() else {
                        return;
                    };
                    app.state.borrow_mut().connecting = false;
                    match result {
                        Ok((server, version, error)) => {
                            let same_cached_account =
                                app.state.borrow().server_address == server_address;
                            if !same_cached_account {
                                app.clear_workspace();
                            }
                            {
                                let mut state = app.state.borrow_mut();
                                state.server = Some(server.clone());
                                if !same_cached_account {
                                    state.organization =
                                        settings::chat_organization(&server_address);
                                }
                                state.server_address = server_address;
                            }
                            app.load_group_avatars();
                            app.set_status(
                                &error
                                    .map(|error| format!("Connected to {version}. {error}"))
                                    .unwrap_or_else(|| format!("Connected to {version}")),
                            );
                            if app.active_guid().is_some() {
                                app.load_messages(Fetch::Latest);
                            } else if !app.restore_last_thread() {
                                app.status_page("Select a conversation", None);
                            }
                            if let Some(prefs) = app.prefs.borrow().as_ref() {
                                prefs.clear_password();
                            }
                            app.refresh_chats(remember.is_some());
                            app.start_realtime(server);
                        }
                        Err(error) => {
                            app.connection_error(&error);
                            if app.state.borrow().server.is_none()
                                && app.state.borrow().chats.is_empty()
                            {
                                app.show_disconnected();
                            } else {
                                app.update_empty_thread();
                            }
                        }
                    }
                    app.update_connection_controls();
                    app.update_composer();
                }
            },
        );
    }

    fn forget(self: &Rc<Self>) {
        if self.state.borrow().connecting || !self.state.borrow().outbox.is_empty() {
            return;
        }
        self.state.borrow_mut().connecting = true;
        self.update_connection_controls();
        self.update_composer();
        background(settings::forget, {
            let weak = Rc::downgrade(self);
            move |result| {
                let Some(app) = weak.upgrade() else {
                    return;
                };
                app.state.borrow_mut().connecting = false;
                match result {
                    Ok(()) => {
                        app.clear_workspace();
                        let mut state = app.state.borrow_mut();
                        state.server = None;
                        state.server_address.clear();
                        drop(state);
                        app.set_status("Not connected");
                        if let Some(prefs) = app.prefs.borrow().as_ref() {
                            prefs.clear_connection();
                        }
                        app.show_disconnected();
                    }
                    Err(error) => app.connection_error(&error),
                }
                app.update_connection_controls();
                app.update_composer();
            }
        });
    }

    fn set_status(&self, status: &str) {
        {
            let mut state = self.state.borrow_mut();
            state.status = status.into();
            state.status_is_error = false;
        }
        if let Some(prefs) = self.prefs.borrow().as_ref() {
            prefs.show_status(status, false);
        }
    }

    fn connection_error(&self, error: &str) {
        {
            let mut state = self.state.borrow_mut();
            state.status = error.into();
            state.status_is_error = true;
        }
        if let Some(prefs) = self.prefs.borrow().as_ref() {
            prefs.show_status(error, true);
        }
        self.notify_error(error);
    }

    fn update_connection_controls(&self) {
        let state = self.state.borrow();
        let busy = state.connecting || !state.outbox.is_empty() || self.mode != Mode::Live;
        if let Some(prefs) = self.prefs.borrow().as_ref() {
            prefs.set_busy(
                busy,
                !settings::saved_server().is_empty() || state.server.is_some(),
            );
        }
        let syncing = state.connecting || state.show_refresh;
        self.refresh
            .set_sensitive(self.mode == Mode::Live && !syncing && state.server.is_some());
        self.refresh_content
            .set_visible_child_name(if syncing { "syncing" } else { "idle" });
        if syncing {
            self.refresh.add_css_class("syncing");
        } else {
            self.refresh.remove_css_class("syncing");
        }
        self.refresh
            .update_state(&[gtk::accessible::State::Busy(syncing)]);
    }

    fn clear_workspace(&self) {
        self.realtime.replace(None);
        self.close_thread();
        self.saved_scroll.borrow_mut().clear();
        let current = self.view();
        for (_, view) in self.timeline_cache.borrow_mut().drain(..) {
            if !Rc::ptr_eq(&view, &current) {
                self.timeline_pages.remove(&view.scroll);
            }
        }
        current.store.remove_all();
        current.restore_offset.set(None);
        current.follow_latest.set(true);
        self.deferred_previews.borrow_mut().clear();
        {
            let mut state = self.state.borrow_mut();
            state.chats.clear();
            state.organization = ChatOrganization::default();
            state.drafts.clear();
            state.threads.clear();
            state.outbox.clear();
            state.previews.clear();
            state.viewer_images.clear();
            state.group_avatars.clear();
            state.loading_previews.clear();
            state.session += 1;
            state.refreshing = false;
            state.show_refresh = false;
            state.last_sidebar_check = None;
            state.live = false;
            state.was_live = false;
            state.seen_notifications.clear();
            state.live_during_refresh.clear();
        }
        self.render_organized_chats();
    }

    fn close_thread(&self) {
        {
            let mut state = self.state.borrow_mut();
            if let Some(previous) = state.active.take() {
                let draft = Draft {
                    text: self.editor_text(),
                    files: std::mem::take(&mut state.files),
                    reply: state.reply.take(),
                };
                state.drafts.insert(previous.guid, draft);
            }
        }
        self.active_highlight.set_text("");
        self.editor.buffer().set_text("");
        self.compose.set_visible(false);
        self.thread_title.set_title("BlueBubbles Native");
        self.thread_avatar.set_visible(false);
        self.layout.show_sidebar();
    }

    fn show_disconnected(&self) {
        self.status_page(
            "Connect to BlueBubbles",
            Some("Enter your server URL and password to see your conversations."),
        );
    }

    fn status_page(&self, title: &str, description: Option<&str>) {
        self.cancel_loading_indicator();
        self.update_connect_actions();
        self.thread_status.set_title(title);
        self.thread_status.set_description(description);
        self.thread_stack.set_visible_child_name("status");
    }

    fn update_connect_actions(&self) {
        let disconnected = self.mode == Mode::Live && self.state.borrow().server.is_none();
        self.connect_button.set_visible(disconnected);
        self.sidebar_connect.set_visible(
            disconnected && self.layout.widget.visible_child_name().as_deref() == Some("narrow"),
        );
    }

    fn active_guid(&self) -> Option<String> {
        self.state
            .borrow()
            .active
            .as_ref()
            .map(|conversation| conversation.guid.clone())
    }

    fn update_active_highlight(&self) {
        let guid = if self.layout.shows_thread() {
            self.active_guid()
        } else {
            None
        };
        self.active_highlight
            .set_text(guid.as_deref().unwrap_or(""));
    }

    fn notify(&self, message: &str) {
        self.show_toast(adw::Toast::new(message));
    }

    fn notify_error(&self, message: &str) {
        self.show_toast(theme::error_toast(message));
    }

    fn show_toast(&self, toast: adw::Toast) {
        if let Some(prefs) = self.prefs.borrow().as_ref() {
            prefs.toast(toast);
        } else {
            self.overlays.add_toast(toast);
        }
    }
}

fn send_background<T: Send + 'static>(
    send: impl FnOnce() -> Result<T, String> + Send + 'static,
    done: impl FnOnce(Result<T, String>) + 'static,
) {
    // An unresponsive send must not occupy any of the cache or sync workers.
    let (sender, receiver) = async_channel::bounded(1);
    let worker = std::thread::Builder::new()
        .name("bluebubbles-send".into())
        .spawn(move || {
            let _ = sender.send_blocking(send());
        });
    glib::spawn_future_local(async move {
        let result = if worker.is_ok() {
            receiver
                .recv()
                .await
                .unwrap_or_else(|_| Err("The send did not finish.".into()))
        } else {
            Err("Could not start the send.".into())
        };
        done(result);
    });
}

fn background<T, Job, Done>(job: Job, done: Done)
where
    T: Send + 'static,
    Job: FnOnce() -> Result<T, String> + Send + 'static,
    Done: FnOnce(Result<T, String>) + 'static,
{
    glib::spawn_future_local(async move {
        let result = gio::spawn_blocking(job)
            .await
            .unwrap_or_else(|_| Err("The operation did not finish.".into()));
        done(result);
    });
}

fn preview_background<T, Job, Done>(job: Job, done: Done)
where
    T: Send + 'static,
    Job: FnOnce() -> Result<T, String> + Send + 'static,
    Done: FnOnce(Result<T, String>) + 'static,
{
    // Slow thumbnails and link metadata must never occupy the message workers.
    static PREVIEWS: OnceLock<Option<glib::ThreadPool>> = OnceLock::new();
    let Some(pool) = PREVIEWS.get_or_init(|| glib::ThreadPool::shared(Some(3)).ok()) else {
        done(Err("Could not start the preview worker.".into()));
        return;
    };
    match pool.push_future(job) {
        Ok(result) => {
            glib::spawn_future_local(async move {
                done(
                    result
                        .await
                        .unwrap_or_else(|_| Err("Could not load the preview.".into())),
                );
            });
        }
        Err(_) => done(Err("Could not queue the preview.".into())),
    }
}

fn boxed<T: 'static>(items: Vec<T>) -> Vec<glib::BoxedAnyObject> {
    items.into_iter().map(glib::BoxedAnyObject::new).collect()
}

fn toolbar(
    title: &impl IsA<gtk::Widget>,
    content: &impl IsA<gtk::Widget>,
) -> (adw::ToolbarView, adw::HeaderBar) {
    let header = adw::HeaderBar::builder().title_widget(title).build();
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(content));
    (toolbar, header)
}

fn scroller(child: &impl IsA<gtk::Widget>) -> gtk::ScrolledWindow {
    gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .overlay_scrolling(true)
        .child(child)
        .build()
}
