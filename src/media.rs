use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;

use adw::prelude::*;
use gtk::gdk_pixbuf::Pixbuf;
use gtk::{gdk, gio, glib, pango};
use image::{ImageDecoder, ImageFormat, ImageReader, metadata::Orientation};
use reqwest::Url;

use crate::icons::Icon;
use crate::model::Attachment;

pub type Handler = Rc<RefCell<Option<Box<dyn Fn(Request)>>>>;

pub enum Request {
    Save(Attachment),
    Open(Attachment, Option<gdk::Texture>),
    Thumbnail(Attachment, InlinePreview),
    Copy(Attachment),
    Link(Url, LinkPreviewTarget),
}

pub struct LinkPreviewTarget {
    card: glib::WeakRef<gtk::LinkButton>,
    files: glib::WeakRef<gtk::Box>,
    title: glib::WeakRef<gtk::Label>,
    description: glib::WeakRef<gtk::Label>,
    reveal_when_rich: bool,
}

impl LinkPreviewTarget {
    pub fn finish(self, host: &str, preview: crate::links::Preview) {
        let has_title = !preview.title.is_empty() && !preview.title.eq_ignore_ascii_case(host);
        let has_description = preview
            .description
            .as_deref()
            .is_some_and(|text| !text.is_empty());
        if has_title && let Some(label) = self.title.upgrade() {
            label.set_label(&preview.title);
            label.set_visible(true);
        }
        if let (Some(description), Some(text)) = (self.description.upgrade(), preview.description)
            && !text.is_empty()
        {
            description.set_label(&text);
            description.set_visible(true);
        }
        if self.reveal_when_rich && (has_title || has_description) {
            if let Some(card) = self.card.upgrade() {
                card.set_visible(true);
            }
            if let Some(files) = self.files.upgrade() {
                files.set_visible(true);
            }
        }
    }
}

pub struct InlinePreview {
    picture: glib::WeakRef<gtk::Picture>,
    placeholder: glib::WeakRef<gtk::Stack>,
}

impl InlinePreview {
    pub fn near_viewport(&self, viewport: &gtk::ScrolledWindow) -> Option<bool> {
        let picture = self.picture.upgrade()?;
        Some(
            picture.is_mapped()
                && picture.compute_bounds(viewport).is_some_and(|bounds| {
                    bounds.height() > 0.0
                        && bounds.y() < viewport.height() as f32 + 80.0
                        && bounds.y() + bounds.height() > -80.0
                }),
        )
    }

    pub fn finish(self, texture: Option<&gdk::Texture>) {
        if let Some(picture) = self.picture.upgrade() {
            picture.set_paintable(texture);
        }
        if let Some(placeholder) = self.placeholder.upgrade() {
            placeholder.set_visible_child_name("fallback");
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    Image,
    Video,
    Audio,
    Other,
}

pub fn image_texture(path: &Path, max_edge: Option<i32>) -> Result<gdk::Texture, glib::Error> {
    let bitmap = match max_edge {
        Some(edge) => Pixbuf::from_file_at_scale(path, edge, edge, true),
        None => Pixbuf::from_file(path),
    };
    let Ok(bitmap) = bitmap else {
        return gdk::Texture::from_filename(path);
    };

    // Older pixbuf loaders may decode PNG or WebP without exposing their EXIF tag.
    if bitmap.option("orientation").is_none()
        && let Some(orientation) = extra_orientation(path)
        && orientation != Orientation::NoTransforms
    {
        bitmap.set_option("orientation", &orientation.to_exif().to_string());
    }
    let upright = bitmap.apply_embedded_orientation().unwrap_or(bitmap);
    Ok(gdk::Texture::for_pixbuf(&upright))
}

fn extra_orientation(path: &Path) -> Option<Orientation> {
    let reader = ImageReader::open(path).ok()?.with_guessed_format().ok()?;
    if !matches!(reader.format(), Some(ImageFormat::Png | ImageFormat::WebP)) {
        return None;
    }
    reader.into_decoder().ok()?.orientation().ok()
}

impl Kind {
    pub fn of(file: &Attachment) -> Self {
        let extension = Path::new(&file.name)
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let mime = file.mime_type.as_deref().unwrap_or("");
        if mime.starts_with("image/")
            || matches!(
                extension.as_str(),
                "jpg" | "jpeg" | "png" | "gif" | "heic" | "heif" | "webp" | "avif"
            )
        {
            Self::Image
        } else if mime.starts_with("video/")
            || matches!(extension.as_str(), "mp4" | "mov" | "webm" | "m4v" | "mkv")
        {
            Self::Video
        } else if mime.starts_with("audio/")
            || matches!(
                extension.as_str(),
                "m4a" | "mp3" | "wav" | "aac" | "flac" | "caf" | "opus"
            )
        {
            Self::Audio
        } else {
            Self::Other
        }
    }

    fn icon(self) -> Icon {
        match self {
            Self::Image => Icon::Image,
            Self::Video => Icon::Video,
            Self::Audio => Icon::Audio,
            Self::Other => Icon::Attach,
        }
    }
}

pub fn attachment_row(
    attachment: &Attachment,
    handler: &Handler,
    message_menu: Option<&gtk::Popover>,
) -> gtk::Widget {
    let kind = Kind::of(attachment);
    if matches!(kind, Kind::Image | Kind::Video) {
        return media_bubble(attachment, kind, handler, message_menu).upcast();
    }

    let row = gtk::Box::new(gtk::Orientation::Horizontal, 7);
    row.add_css_class("attachment-row");
    row.append(&kind.icon().widget());
    let name = gtk::Label::builder()
        .label(&attachment.name)
        .ellipsize(pango::EllipsizeMode::Middle)
        .max_width_chars(26)
        .hexpand(true)
        .xalign(0.0)
        .single_line_mode(true)
        .build();
    row.append(&name);
    if kind == Kind::Audio {
        let play = Icon::Play.button(&format!("Play {}", attachment.name));
        play.add_css_class("flat");
        row.append(&play);
        play.connect_clicked({
            let handler = handler.clone();
            let attachment = attachment.clone();
            move |_| emit(&handler, Request::Open(attachment.clone(), None))
        });
    }
    let save = Icon::Download.button(&format!("Download {}", attachment.name));
    save.add_css_class("flat");
    row.append(&save);
    save.connect_clicked({
        let handler = handler.clone();
        let attachment = attachment.clone();
        move |_| emit(&handler, Request::Save(attachment.clone()))
    });
    row.upcast()
}

fn media_bubble(
    attachment: &Attachment,
    kind: Kind,
    handler: &Handler,
    message_menu: Option<&gtk::Popover>,
) -> gtk::Overlay {
    let picture = gtk::Picture::builder()
        .can_shrink(true)
        .content_fit(gtk::ContentFit::Cover)
        .width_request(236)
        .height_request(148)
        .build();
    picture.set_overflow(gtk::Overflow::Hidden);
    let visual = gtk::Overlay::new();
    visual.set_child(Some(&picture));
    let placeholder = gtk::Stack::new();
    placeholder.set_valign(gtk::Align::Center);
    placeholder.set_halign(gtk::Align::Center);
    placeholder.set_can_target(false);
    placeholder.add_css_class("media-placeholder");
    placeholder.add_named(&kind.icon().widget(), Some("fallback"));
    placeholder.add_named(&crate::loading::orbit(22), Some("loading"));
    visual.add_overlay(&placeholder);
    let weak_placeholder = placeholder.downgrade();
    picture.connect_paintable_notify(move |picture| {
        if let Some(media) = picture.paintable() {
            let ratio = media.intrinsic_aspect_ratio();
            if ratio.is_finite() && ratio > 0.0 {
                let max_height = if kind == Kind::Image { 320.0 } else { 240.0 };
                let width = (max_height * ratio).clamp(72.0, 236.0);
                let height = (width / ratio).clamp(24.0, max_height);
                picture.set_size_request(width.round() as i32, height.round() as i32);
            }
        }
        if let Some(placeholder) = weak_placeholder.upgrade() {
            placeholder.set_visible(picture.paintable().is_none());
        }
    });

    let card = gtk::Button::builder()
        .child(&crate::squircle::Squircle::new(&visual, "media-contour"))
        .build();
    card.add_css_class("media-thumbnail");
    card.set_cursor_from_name(Some(if kind == Kind::Image {
        "zoom-in"
    } else {
        "pointer"
    }));
    card.update_property(&[gtk::accessible::Property::Label(&format!(
        "Open {}",
        attachment.name
    ))]);
    card.connect_clicked({
        let handler = handler.clone();
        let attachment = attachment.clone();
        let picture = picture.downgrade();
        move |_| emit(&handler, open_request(&attachment, Some(&picture)))
    });
    card.connect_map({
        let handler = handler.clone();
        let attachment = attachment.clone();
        let picture = picture.downgrade();
        let requested = Cell::new(false);
        move |_| {
            if !requested.replace(true) {
                placeholder.set_visible_child_name("loading");
                emit(
                    &handler,
                    Request::Thumbnail(
                        attachment.clone(),
                        InlinePreview {
                            picture: picture.clone(),
                            placeholder: placeholder.downgrade(),
                        },
                    ),
                );
            }
        }
    });

    media_context(&card, attachment, handler, Some(&picture), message_menu)
}

fn open_request(attachment: &Attachment, preview: Option<&glib::WeakRef<gtk::Picture>>) -> Request {
    let texture = preview
        .and_then(glib::WeakRef::upgrade)
        .and_then(|picture| picture.paintable().and_downcast::<gdk::Texture>());
    Request::Open(attachment.clone(), texture)
}

fn media_context(
    target: &impl IsA<gtk::Widget>,
    attachment: &Attachment,
    handler: &Handler,
    preview: Option<&gtk::Picture>,
    message_menu: Option<&gtk::Popover>,
) -> gtk::Overlay {
    let actions = gio::SimpleActionGroup::new();
    let mut items = Vec::new();
    for (label, name, detailed_action) in [
        ("Open in Viewer", "open", "media.open"),
        ("Copy", "copy", "media.copy"),
        ("Download", "download", "media.download"),
    ] {
        if name == "open" && preview.is_none() {
            continue;
        }
        let action = gio::SimpleAction::new(name, None);
        let handler = handler.clone();
        let attachment = attachment.clone();
        let preview = preview.map(ObjectExt::downgrade);
        action.connect_activate(move |_, _| {
            let request = match name {
                "open" => open_request(&attachment, preview.as_ref()),
                "copy" => Request::Copy(attachment.clone()),
                _ => Request::Save(attachment.clone()),
            };
            emit(&handler, request);
        });
        actions.add_action(&action);
        items.push((label, detailed_action, None));
    }
    if let Some(menu) = message_menu {
        let menu = menu.downgrade();
        let action = gio::SimpleAction::new("message", None);
        action.connect_activate(move |_, _| {
            let menu = menu.clone();
            glib::idle_add_local_once(move || {
                if let Some(menu) = menu.upgrade() {
                    crate::context_menu::popup(&menu, Some((12.0, 12.0)));
                }
            });
        });
        actions.add_action(&action);
        items.push(("Message Actions", "media.message", None));
    }

    let root = gtk::Overlay::new();
    root.set_child(Some(target));
    root.insert_action_group("media", Some(&actions));
    let popover = crate::context_menu::attach(&root, target);
    crate::context_menu::set_commands(&popover, &items);
    let keyboard = gtk::EventControllerKey::new();
    let popover = popover.downgrade();
    keyboard.connect_key_pressed(move |_, key, _, modifiers| {
        if crate::context_menu::keyboard_shortcut(key, modifiers)
            && let Some(popover) = popover.upgrade()
        {
            crate::context_menu::popup(&popover, None);
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    target.add_controller(keyboard);
    root
}

pub fn thumbnail(path: &Path, kind: Kind) -> Result<std::path::PathBuf, String> {
    if kind != Kind::Video {
        return Ok(path.to_owned());
    }
    let poster = path.with_extension("poster.png");
    if poster.is_file() {
        return Ok(poster);
    }
    let directory = poster
        .parent()
        .ok_or("Could not create a local video preview.")?;
    let file = tempfile::Builder::new()
        .suffix(".png")
        .tempfile_in(directory)
        .map_err(|_| "Could not create a local video preview.".to_string())?;
    let status = std::process::Command::new("ffmpeg")
        .args([
            "-nostdin",
            "-loglevel",
            "error",
            "-y",
            "-protocol_whitelist",
            "file,pipe",
            "-i",
        ])
        .arg(path)
        .args([
            "-frames:v",
            "1",
            "-vf",
            "scale=480:480:force_original_aspect_ratio=decrease",
            "-update",
            "1",
        ])
        .arg(file.path())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|_| "Could not prepare a video preview.".to_string())?;
    if !status.success() {
        return Err("Could not prepare a video preview.".into());
    }
    file.persist(&poster)
        .map_err(|_| "Could not save a local video preview.".to_string())?;
    Ok(poster)
}

pub fn copy_to_clipboard(
    window: &adw::ApplicationWindow,
    path: &Path,
    image: Option<&gdk::Texture>,
) -> Result<(), String> {
    let clipboard = RootExt::display(window).clipboard();
    if let Some(image) = image {
        clipboard.set_texture(image);
    } else {
        let files = gdk::FileList::from_array(&[gio::File::for_path(path)]);
        let provider = gdk::ContentProvider::for_value(&files.to_value());
        clipboard
            .set_content(Some(&provider))
            .map_err(|_| "The file could not be copied.".to_string())?;
    }
    Ok(())
}

pub fn link_row(
    url: Url,
    outgoing: bool,
    standalone: bool,
    files: &gtk::Box,
    handler: &Handler,
) -> gtk::LinkButton {
    let host = url.host_str().unwrap_or("Website");
    let text = gtk::Box::new(gtk::Orientation::Vertical, 3);
    let hostname = gtk::Label::builder()
        .label(host)
        .xalign(0.0)
        .single_line_mode(true)
        .ellipsize(pango::EllipsizeMode::End)
        .max_width_chars(42)
        .build();
    hostname.add_css_class("dim-label");
    let label = gtk::Label::builder()
        .xalign(0.0)
        .max_width_chars(42)
        .single_line_mode(true)
        .ellipsize(pango::EllipsizeMode::End)
        .visible(false)
        .build();
    label.add_css_class("heading");
    let details = gtk::Label::builder()
        .xalign(0.0)
        .max_width_chars(42)
        .wrap(true)
        .lines(2)
        .ellipsize(pango::EllipsizeMode::End)
        .visible(false)
        .build();
    details.add_css_class("dim-label");
    text.append(&hostname);
    text.append(&label);
    text.append(&details);
    let link = gtk::LinkButton::new(url.as_str());
    link.set_child(Some(&text));
    link.update_property(&[gtk::accessible::Property::Label(&format!("Open {host}"))]);
    link.add_css_class("link-card");
    if outgoing {
        link.add_css_class("outgoing");
    }
    let make_target = {
        let card = link.downgrade();
        let files = files.downgrade();
        let title = label.downgrade();
        let description = details.downgrade();
        move || LinkPreviewTarget {
            card: card.clone(),
            files: files.clone(),
            title: title.clone(),
            description: description.clone(),
            reveal_when_rich: !standalone,
        }
    };
    if standalone {
        link.connect_map({
            let handler = handler.clone();
            let requested = Cell::new(false);
            move |_| {
                if !requested.replace(true) {
                    emit(&handler, Request::Link(url.clone(), make_target()));
                }
            }
        });
    } else {
        // Embedded links are already clickable inline inside the message bubble;
        // only reveal the preview card if rich metadata is available.
        link.set_visible(false);
        emit(handler, Request::Link(url, make_target()));
    }
    link
}

fn viewer_dimensions(parent: &adw::ApplicationWindow) -> (i32, i32) {
    (
        (parent.width() - 76).clamp(340, 1150),
        (parent.height() - 72).clamp(340, 840),
    )
}

pub fn viewer_image_edge(parent: &adw::ApplicationWindow) -> i32 {
    let (width, height) = viewer_dimensions(parent);
    width.max(height) * parent.scale_factor()
}

pub fn open_viewer(
    parent: &adw::ApplicationWindow,
    attachment: &Attachment,
    preview: Option<&gdk::Texture>,
    handler: &Handler,
) -> glib::WeakRef<gtk::Overlay> {
    let _profile = crate::profile::span("viewer-present-ui");
    let viewer = gtk::Overlay::new();
    viewer.add_css_class("bb-lightbox");
    if let Some(texture) = preview {
        set_viewer_image(&viewer, attachment, texture, handler);
    } else {
        let weak = viewer.downgrade();
        glib::timeout_add_local_once(std::time::Duration::from_millis(450), move || {
            if let Some(viewer) = weak.upgrade()
                && viewer.child().is_none()
            {
                viewer.set_child(Some(&crate::loading::state("Loading media…")));
            }
        });
    }
    let close = Icon::Remove.button("Close media viewer");
    if let Some(icon) = close.child() {
        icon.set_halign(gtk::Align::Fill);
        icon.set_valign(gtk::Align::Fill);
    }
    close.add_css_class("circular");
    close.add_css_class("viewer-close");
    close.set_halign(gtk::Align::End);
    close.set_valign(gtk::Align::Start);
    close.set_margin_top(12);
    close.set_margin_end(12);
    viewer.add_overlay(&close);
    let (width, height) = viewer_dimensions(parent);
    let dialog = adw::Dialog::builder()
        .title(&attachment.name)
        .presentation_mode(adw::DialogPresentationMode::Floating)
        .content_width(width)
        .content_height(height)
        .child(&viewer)
        .build();
    let weak = dialog.downgrade();
    close.connect_clicked(move |_| {
        if let Some(dialog) = weak.upgrade() {
            dialog.close();
        }
    });
    viewer.connect_map(|viewer| crate::profile::sample_frames(viewer, "viewer-opening-frames"));
    dialog.present(Some(parent));
    viewer.downgrade()
}

pub fn show_viewer(
    viewer: &gtk::Overlay,
    attachment: &Attachment,
    path: &Path,
    texture: Option<gdk::Texture>,
    handler: &Handler,
) {
    let _profile = crate::profile::span("viewer-content-ui");
    let kind = Kind::of(attachment);
    if let Some(texture) = texture {
        set_viewer_image(viewer, attachment, &texture, handler);
    } else {
        set_viewer_content(
            viewer,
            attachment,
            &playback(kind, path),
            kind != Kind::Audio,
            handler,
        );
    }
}

fn set_viewer_image(
    viewer: &gtk::Overlay,
    attachment: &Attachment,
    texture: &gdk::Texture,
    handler: &Handler,
) {
    let current = viewer
        .child()
        .and_downcast::<gtk::Overlay>()
        .and_then(|content| content.child().and_downcast::<gtk::Picture>());
    if let Some(picture) = current {
        picture.set_paintable(Some(texture));
        return;
    }
    let picture = gtk::Picture::builder()
        .paintable(texture)
        .can_shrink(true)
        .content_fit(gtk::ContentFit::Contain)
        .build();
    set_viewer_content(viewer, attachment, &picture, true, handler);
}

fn set_viewer_content(
    viewer: &gtk::Overlay,
    attachment: &Attachment,
    widget: &impl IsA<gtk::Widget>,
    expand: bool,
    handler: &Handler,
) {
    widget.set_hexpand(true);
    widget.set_vexpand(expand);
    widget.set_focusable(true);
    widget
        .as_ref()
        .update_property(&[gtk::accessible::Property::Label(&attachment.name)]);
    viewer.set_child(Some(&media_context(
        widget, attachment, handler, None, None,
    )));
}

pub fn show_viewer_error(viewer: &gtk::Overlay, error: &str) {
    let message = adw::StatusPage::builder()
        .title("Could not open media")
        .description(error)
        .build();
    message.add_css_class("compact");
    viewer.set_child(Some(&message));
}

fn playback(kind: Kind, path: &Path) -> gtk::Widget {
    if kind == Kind::Audio {
        let audio = gtk::MediaFile::for_filename(path);
        let player = gtk::MediaControls::new(Some(&audio));
        audio.play();
        player.upcast()
    } else {
        let video = gtk::Video::for_filename(Some(path));
        video.set_autoplay(true);
        video.upcast()
    }
}

fn emit(handler: &Handler, request: Request) {
    if let Some(handle) = handler.borrow().as_ref() {
        handle(request);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{
        ExtendedColorType, ImageEncoder, codecs::jpeg::JpegEncoder, codecs::png::PngEncoder,
    };

    #[test]
    fn image_textures_follow_exif_in_previews_and_originals() {
        let directory = tempfile::tempdir().unwrap();
        let colors = [[230, 22, 25], [33, 219, 39], [27, 42, 227], [229, 214, 22]];
        let (width, height) = (80, 48);
        let pixels: Vec<_> = (0..height)
            .flat_map(|y| {
                (0..width).flat_map(move |x| {
                    colors[usize::from(y >= height / 2) * 2 + usize::from(x >= width / 2)]
                })
            })
            .collect();
        let corners = [
            [0, 1, 2, 3],
            [1, 0, 3, 2],
            [3, 2, 1, 0],
            [2, 3, 0, 1],
            [0, 2, 1, 3],
            [2, 0, 3, 1],
            [3, 1, 2, 0],
            [1, 3, 0, 2],
        ];

        for (index, expected) in corners.into_iter().enumerate() {
            let tag = index as u8 + 1;
            for extension in ["jpg", "png"] {
                let path = directory
                    .path()
                    .join(format!("orientation-{tag}.{extension}"));
                let output = std::fs::File::create(&path).unwrap();
                let exif = vec![
                    b'I', b'I', 42, 0, 8, 0, 0, 0, 1, 0, 0x12, 1, 3, 0, 1, 0, 0, 0, tag, 0, 0, 0,
                    0, 0, 0, 0,
                ];
                if extension == "jpg" {
                    encode(
                        JpegEncoder::new_with_quality(output, 100),
                        exif,
                        &pixels,
                        (width, height),
                    );
                } else {
                    encode(PngEncoder::new(output), exif, &pixels, (width, height));
                }
                if extension == "png" {
                    assert_eq!(extra_orientation(&path), Orientation::from_exif(tag));
                }
                for max_edge in [None, Some(40)] {
                    let texture = image_texture(&path, max_edge).unwrap();
                    let (w, h) = if max_edge.is_some() {
                        (40, 24)
                    } else {
                        (80, 48)
                    };
                    let (w, h) = if tag >= 5 { (h, w) } else { (w, h) };
                    let rendered = image::load_from_memory(texture.save_to_png_bytes().as_ref())
                        .unwrap()
                        .to_rgb8();
                    assert_eq!(rendered.dimensions(), (w, h), "{extension} EXIF {tag}");
                    let points = [
                        (w / 4, h / 4),
                        (3 * w / 4, h / 4),
                        (w / 4, 3 * h / 4),
                        (3 * w / 4, 3 * h / 4),
                    ];
                    for ((x, y), color) in
                        points.into_iter().zip(expected.map(|index| colors[index]))
                    {
                        let actual = rendered.get_pixel(x, y).0;
                        assert!(
                            actual
                                .into_iter()
                                .zip(color)
                                .all(|(a, b)| a.abs_diff(b) <= 8),
                            "{extension} EXIF {tag} at ({x}, {y}): {actual:?} != {color:?}"
                        );
                    }
                }
            }
        }
    }

    fn encode(
        mut encoder: impl ImageEncoder,
        exif: Vec<u8>,
        pixels: &[u8],
        (width, height): (u32, u32),
    ) {
        encoder.set_exif_metadata(exif).unwrap();
        encoder
            .write_image(pixels, width, height, ExtendedColorType::Rgb8)
            .unwrap();
    }
}
