use adw::prelude::*;
use gtk::{gdk, glib, prelude::RootExt};

pub fn attach(host: &gtk::Overlay, target: &impl IsA<gtk::Widget>) -> gtk::Popover {
    let popover = gtk::Popover::new();
    popover.add_css_class("context-menu");
    popover.set_offset(0, 8);
    let commands = gtk::Box::new(gtk::Orientation::Vertical, 1);
    commands.add_css_class("context-commands");
    commands.set_focusable(true);
    popover.set_child(Some(&commands));
    host.add_overlay(&popover);

    let keyboard = gtk::EventControllerKey::new();
    keyboard.set_propagation_phase(gtk::PropagationPhase::Capture);
    let weak = popover.downgrade();
    keyboard.connect_key_pressed(move |_, key, _, _| {
        let Some(popover) = weak.upgrade() else {
            return glib::Propagation::Proceed;
        };
        let Some(commands) = popover.child() else {
            return glib::Propagation::Proceed;
        };
        match key {
            gdk::Key::Down
            | gdk::Key::Up
            | gdk::Key::Left
            | gdk::Key::Right
            | gdk::Key::Home
            | gdk::Key::End => {
                let buttons = menu_buttons(&commands);
                let focus = popover.root().and_then(|root| root.focus());
                let current = buttons
                    .iter()
                    .position(|button| Some(button) == focus.as_ref());
                let next = match key {
                    gdk::Key::Home => buttons.first(),
                    gdk::Key::End => buttons.last(),
                    gdk::Key::Down | gdk::Key::Right => current
                        .and_then(|index| buttons.get(index + 1))
                        .or_else(|| buttons.first()),
                    _ => current
                        .and_then(|index| index.checked_sub(1).and_then(|index| buttons.get(index)))
                        .or_else(|| buttons.last()),
                };
                if let Some(button) = next {
                    button.grab_focus();
                }
                glib::Propagation::Stop
            }
            gdk::Key::Return | gdk::Key::KP_Enter | gdk::Key::space if commands.has_focus() => {
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        }
    });
    popover.add_controller(keyboard);

    bind_pointer(&popover, target, gtk::PropagationPhase::Bubble);
    popover
}

pub fn bind_pointer(
    popover: &gtk::Popover,
    target: &impl IsA<gtk::Widget>,
    phase: gtk::PropagationPhase,
) {
    let context = gtk::GestureClick::new();
    context.set_button(3);
    context.set_propagation_phase(phase);
    let weak = popover.downgrade();
    context.connect_pressed(move |gesture, _, x, y| {
        if let Some(popover) = weak.upgrade() {
            gesture.set_state(gtk::EventSequenceState::Claimed);
            let position = gesture
                .widget()
                .zip(popover.parent())
                .and_then(|(source, parent)| {
                    source.compute_point(&parent, &gtk::graphene::Point::new(x as f32, y as f32))
                })
                .map_or((x, y), |point| (f64::from(point.x()), f64::from(point.y())));
            popup(&popover, Some(position));
        }
    });
    target.add_controller(context);
}

pub fn set_commands(popover: &gtk::Popover, entries: &[(&str, &str, Option<&glib::Variant>)]) {
    let Some(commands) = popover.child().and_downcast::<gtk::Box>() else {
        return;
    };
    if let Some(root) = commands.root()
        && let Some(focus) = root.focus()
        && focus.is_ancestor(&commands)
    {
        root.set_focus(None::<&gtk::Widget>);
    }
    while let Some(child) = commands.first_child() {
        commands.remove(&child);
    }
    for &(label, action, target) in entries {
        let text = gtk::Label::builder()
            .label(label)
            .xalign(0.0)
            .hexpand(true)
            .build();
        commands.append(&command_button(popover, &text, action, target));
    }
}

pub fn prepend_reactions(
    popover: &gtk::Popover,
    guid: &str,
    selected: Option<crate::model::TapbackKind>,
) {
    let Some(commands) = popover.child().and_downcast::<gtk::Box>() else {
        return;
    };
    let grid = gtk::Grid::builder()
        .column_spacing(2)
        .column_homogeneous(true)
        .build();
    grid.add_css_class("reaction-picker");
    for (column, kind) in crate::model::TapbackKind::ALL.into_iter().enumerate() {
        let text = gtk::Label::new(Some(kind.emoji()));
        let target = (guid, kind.name()).to_variant();
        let button = command_button(popover, &text, "message.react", Some(&target));
        button.add_css_class("reaction-emoji");
        let title = if Some(kind) == selected {
            button.add_css_class("own");
            format!("Remove {}", kind.label())
        } else {
            kind.label().to_owned()
        };
        button.set_tooltip_text(Some(&title));
        button.update_property(&[gtk::accessible::Property::Label(&title)]);
        grid.attach(&button, column as i32, 0, 1, 1);
    }
    commands.prepend(&grid);
}

fn command_button(
    popover: &gtk::Popover,
    label: &gtk::Label,
    action: &str,
    target: Option<&glib::Variant>,
) -> gtk::Button {
    let button = gtk::Button::builder()
        .child(label)
        .accessible_role(gtk::AccessibleRole::MenuItem)
        .build();
    button.add_css_class("flat");
    button.set_action_target_value(target);
    button.set_action_name(Some(action));
    let weak = popover.downgrade();
    button.connect_clicked(move |_| {
        if let Some(popover) = weak.upgrade() {
            // A command can rebind the row that owns its menu. Move keyboard
            // focus to the list before GTK unmaps the transient controls.
            if let Some(list) = popover.ancestor(gtk::ListView::static_type())
                && let Some(root) = list.root()
            {
                root.set_focus(Some(&list));
            }
            popover.popdown();
        }
    });
    button
}

fn menu_buttons(commands: &gtk::Widget) -> Vec<gtk::Widget> {
    let mut buttons = Vec::new();
    let mut child = commands.first_child();
    while let Some(widget) = child {
        child = widget.next_sibling();
        if let Some(grid) = widget.downcast_ref::<gtk::Grid>() {
            for column in 0..crate::model::TapbackKind::ALL.len() as i32 {
                buttons.extend(grid.child_at(column, 0));
            }
        } else {
            buttons.push(widget);
        }
    }
    buttons.retain(gtk::Widget::is_sensitive);
    buttons
}

pub fn popup(popover: &gtk::Popover, pointer: Option<(f64, f64)>) {
    let point = pointer.map(|(x, y)| gdk::Rectangle::new(x.round() as i32, y.round() as i32, 1, 1));
    popover.set_pointing_to(point.as_ref());
    popover.popup();
    if let Some(commands) = popover.child() {
        if pointer.is_some() {
            commands.grab_focus();
        } else if let Some(first) = menu_buttons(&commands).first() {
            first.grab_focus();
        }
    }
}

pub fn keyboard_shortcut(key: gdk::Key, modifiers: gdk::ModifierType) -> bool {
    key == gdk::Key::Menu
        || (key == gdk::Key::F10 && modifiers.contains(gdk::ModifierType::SHIFT_MASK))
}
