use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;

pub struct Layout {
    pub widget: gtk::Stack,
    navigation: adw::NavigationView,
    sidebar_page: adw::NavigationPage,
    thread_page: adw::NavigationPage,
    thread_open: Rc<Cell<bool>>,
}

impl Layout {
    pub fn new(
        window: &adw::ApplicationWindow,
        sidebar: &adw::ToolbarView,
        sidebar_header: &adw::HeaderBar,
        thread: &adw::ToolbarView,
        thread_header: &adw::HeaderBar,
        remember_width: bool,
    ) -> Self {
        let initial_width = if remember_width {
            crate::settings::sidebar_width()
        } else {
            316
        };
        let paned = gtk::Paned::builder()
            .orientation(gtk::Orientation::Horizontal)
            .wide_handle(true)
            .resize_start_child(false)
            .shrink_start_child(false)
            .shrink_end_child(false)
            .start_child(sidebar)
            .end_child(thread)
            .position(initial_width)
            .build();
        paned.add_css_class("workspace");
        sidebar.set_width_request(205);
        thread.set_width_request(300);
        sidebar_header.set_show_end_title_buttons(false);
        thread_header.set_show_start_title_buttons(false);

        let navigation = adw::NavigationView::new();
        let sidebar_page = adw::NavigationPage::builder()
            .title("Conversations")
            .build();
        let thread_page = adw::NavigationPage::builder().title("Conversation").build();
        navigation.add(&sidebar_page);
        navigation.add(&thread_page);
        let thread_open = Rc::new(Cell::new(false));
        navigation.connect_popped({
            let thread_open = thread_open.clone();
            let thread_page = thread_page.clone();
            move |_, page| {
                if page == &thread_page {
                    thread_open.set(false);
                }
            }
        });

        let widget = gtk::Stack::new();
        widget.set_hhomogeneous(false);
        widget.set_vhomogeneous(false);
        let headers = gtk::SizeGroup::new(gtk::SizeGroupMode::Vertical);
        headers.add_widget(sidebar_header);
        headers.add_widget(thread_header);
        sidebar_header.add_css_class("main-header");
        thread_header.add_css_class("main-header");

        let wide = gtk::Overlay::new();
        // Paint one header and bottom rule under both transparent toolbars.
        let surface = gtk::Box::new(gtk::Orientation::Vertical, 0);
        surface.set_can_target(false);
        let header_background = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        header_background.add_css_class("wide-header-background");
        surface.append(&header_background);
        sidebar.connect_top_bar_height_notify({
            let background = header_background.downgrade();
            move |toolbar| {
                if let Some(background) = background.upgrade() {
                    background.set_height_request(toolbar.top_bar_height());
                }
            }
        });
        wide.set_child(Some(&surface));
        wide.add_overlay(&paned);
        wide.set_measure_overlay(&paned, true);
        widget.add_named(&wide, Some("wide"));
        widget.add_named(&navigation, Some("narrow"));

        let saved_position = Rc::new(Cell::new(initial_width));
        if remember_width {
            let timeout = Rc::new(RefCell::<Option<glib::SourceId>>::new(None));
            paned.connect_position_notify(move |paned| {
                if paned.width() <= 680 || !paned.is_mapped() {
                    return;
                }
                if let Some(previous) = timeout.borrow_mut().take() {
                    previous.remove();
                }
                let width = paned.position();
                let weak = paned.downgrade();
                let pending = timeout.clone();
                let id = glib::timeout_add_local_once(Duration::from_millis(220), move || {
                    pending.borrow_mut().take();
                    if let Some(paned) = weak.upgrade()
                        && paned.is_mapped()
                        && paned.position() == width
                    {
                        crate::settings::save_sidebar_width(width);
                    }
                });
                *timeout.borrow_mut() = Some(id);
            });
        }
        let breakpoint =
            adw::Breakpoint::new(adw::BreakpointCondition::parse("max-width: 680sp").unwrap());
        breakpoint.connect_apply({
            let (
                paned,
                sidebar,
                thread,
                thread_header,
                sidebar_page,
                thread_page,
                widget,
                saved_position,
                navigation,
                thread_open,
            ) = (
                paned.clone(),
                sidebar.clone(),
                thread.clone(),
                thread_header.clone(),
                sidebar_page.clone(),
                thread_page.clone(),
                widget.clone(),
                saved_position.clone(),
                navigation.clone(),
                thread_open.clone(),
            );
            move |_| {
                saved_position.set(paned.position());
                paned.set_start_child(gtk::Widget::NONE);
                paned.set_end_child(gtk::Widget::NONE);
                thread_header.set_show_start_title_buttons(true);
                sidebar_page.set_child(Some(&sidebar));
                thread_page.set_child(Some(&thread));
                if thread_open.get() && navigation.visible_page().as_ref() != Some(&thread_page) {
                    navigation.push(&thread_page);
                }
                widget.set_visible_child_name("narrow");
            }
        });
        breakpoint.connect_unapply({
            let (sidebar_page, thread_page, widget) =
                (sidebar_page.clone(), thread_page.clone(), widget.clone());
            let (sidebar, thread, thread_header) =
                (sidebar.clone(), thread.clone(), thread_header.clone());
            move |_| {
                sidebar_page.set_child(gtk::Widget::NONE);
                thread_page.set_child(gtk::Widget::NONE);
                thread_header.set_show_start_title_buttons(false);
                paned.set_start_child(Some(&sidebar));
                paned.set_end_child(Some(&thread));
                paned.set_position(saved_position.get());
                widget.set_visible_child_name("wide");
            }
        });
        window.add_breakpoint(breakpoint);
        Self {
            widget,
            navigation,
            sidebar_page,
            thread_page,
            thread_open,
        }
    }

    pub fn show_thread(&self) {
        self.thread_open.set(true);
        if self.widget.visible_child_name().as_deref() == Some("narrow")
            && self.navigation.visible_page().as_ref() != Some(&self.thread_page)
        {
            self.navigation.push(&self.thread_page);
        }
    }

    pub fn show_sidebar(&self) {
        self.thread_open.set(false);
        if self.navigation.visible_page().as_ref() == Some(&self.thread_page) {
            self.navigation.pop_to_page(&self.sidebar_page);
        }
    }

    pub fn shows_thread(&self) -> bool {
        self.widget.visible_child_name().as_deref() != Some("narrow")
            || self.navigation.visible_page().as_ref() == Some(&self.thread_page)
    }

    pub fn on_page_changed(&self, callback: impl Fn() + 'static) {
        self.navigation
            .connect_visible_page_notify(move |_| callback());
    }
}

pub fn shortcut(window: &adw::ApplicationWindow, trigger: &str, action: impl Fn() + 'static) {
    let controller = gtk::ShortcutController::new();
    controller.set_scope(gtk::ShortcutScope::Managed);
    controller.add_shortcut(gtk::Shortcut::new(
        gtk::ShortcutTrigger::parse_string(trigger),
        Some(gtk::CallbackAction::new(move |_, _| {
            action();
            glib::Propagation::Stop
        })),
    ));
    window.add_controller(controller);
}

pub const SIDEBAR_INSET: i32 = 8;
pub const SIDEBAR_ROW_GAP: i32 = 2;

pub fn thread_clamp(child: &impl IsA<gtk::Widget>) -> adw::Clamp {
    let clamp = adw::Clamp::builder()
        .maximum_size(820)
        .tightening_threshold(660)
        .build();
    clamp.set_child(Some(child));
    clamp
}
