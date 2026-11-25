use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use adw::prelude::*;
use gtk::{gdk, glib};

use crate::api::BlueBubbles;
use crate::cache::Cache;
use crate::model::Attachment;
use crate::{links, media};

use super::{Workspace, background, preview_background};

struct MediaSource {
    cache: Cache,
    server: Option<BlueBubbles>,
}

impl MediaSource {
    fn media(&self, attachment: &Attachment, original: bool) -> Result<PathBuf, String> {
        if let Some(server) = &self.server {
            return server.media(attachment, original);
        }
        for variant in [original, !original] {
            let path = self
                .cache
                .media_path(&attachment.guid, &attachment.name, variant)?;
            if path.is_file() {
                return Ok(path);
            }
        }
        Err("The attachment isn't cached. Reconnect to load it.".into())
    }

    fn link(&self, url: &reqwest::Url) -> links::Preview {
        if self.server.is_some() {
            links::fetch(url, &self.cache)
        } else {
            crate::cache::read_json(&self.cache.link_path(url.as_str())).unwrap_or_else(|_| {
                links::Preview {
                    title: url.host_str().unwrap_or("Link").into(),
                    description: None,
                }
            })
        }
    }
}

impl Workspace {
    pub(super) fn schedule_previews(self: &Rc<Self>) {
        if self.deferred_previews.borrow().is_empty() || self.preview_check.replace(true) {
            return;
        }
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(Duration::from_millis(28), move || {
            let Some(app) = weak.upgrade() else { return };
            app.preview_check.set(false);
            let view = app.view();
            let mut waiting = Vec::new();
            let mut ready = Vec::new();
            for (attachment, slot) in app.deferred_previews.take() {
                match slot.near_viewport(&view.scroll) {
                    Some(true) => ready.push((attachment, slot)),
                    Some(false) => waiting.push((attachment, slot)),
                    None => {}
                }
            }
            app.deferred_previews.replace(waiting);
            for (attachment, slot) in ready {
                app.load_media(media::Request::Thumbnail(attachment, slot));
            }
        });
    }

    pub(super) fn media_request(self: &Rc<Self>, request: media::Request) {
        match request {
            media::Request::Save(attachment) => self.download(attachment),
            media::Request::Thumbnail(attachment, slot) => {
                self.deferred_previews.borrow_mut().push((attachment, slot));
                self.schedule_previews();
            }
            request => self.load_media(request),
        }
    }

    fn load_media(self: &Rc<Self>, request: media::Request) {
        let source = {
            let state = self.state.borrow();
            if state.server_address.is_empty() {
                if let media::Request::Thumbnail(_, slot) = request {
                    slot.finish(None);
                }
                return;
            }
            MediaSource {
                cache: Cache::new(&state.server_address),
                server: state.server.clone(),
            }
        };
        match request {
            media::Request::Thumbnail(attachment, slot) => {
                let guid = attachment.guid.clone();
                let session = {
                    let mut state = self.state.borrow_mut();
                    if let Some(index) = state.previews.iter().position(|(id, _)| id == &guid) {
                        let entry = state.previews.remove(index).expect("Known preview");
                        let texture = entry.1.clone();
                        state.previews.push_back(entry);
                        drop(state);
                        slot.finish(Some(&texture));
                        return;
                    }
                    if let Some(waiting) = state.loading_previews.get_mut(&guid) {
                        waiting.push(slot);
                        return;
                    }
                    state.loading_previews.insert(guid.clone(), vec![slot]);
                    state.session
                };
                let kind = media::Kind::of(&attachment);
                preview_background(
                    move || {
                        let _profile = crate::profile::span("preview-worker");
                        let path = media::thumbnail(&source.media(&attachment, false)?, kind)?;
                        media::image_texture(&path, Some(640)).map_err(|error| error.to_string())
                    },
                    {
                        let weak = Rc::downgrade(self);
                        move |result: Result<gdk::Texture, String>| {
                            let Some(app) = weak.upgrade() else { return };
                            let texture = result.ok();
                            let pending = {
                                let mut state = app.state.borrow_mut();
                                if state.session != session {
                                    return;
                                }
                                if let Some(texture) = texture.as_ref()
                                    && i64::from(texture.width()) * i64::from(texture.height())
                                        <= 640 * 640
                                {
                                    if state.previews.len() >= 32 {
                                        state.previews.pop_front();
                                    }
                                    state.previews.push_back((guid.clone(), texture.clone()));
                                }
                                state.loading_previews.remove(&guid).unwrap_or_default()
                            };
                            for slot in pending {
                                slot.finish(texture.as_ref());
                            }
                        }
                    },
                );
            }
            media::Request::Copy(attachment) => {
                let Some(window) = self.window.upgrade() else {
                    return;
                };
                background(
                    move || {
                        let path = source.media(&attachment, true)?;
                        let image = if media::Kind::of(&attachment) == media::Kind::Image {
                            Some(media::image_texture(&path, None).map_err(|_| {
                                "This image format could not be copied.".to_string()
                            })?)
                        } else {
                            None
                        };
                        Ok((path, image))
                    },
                    {
                        let weak = Rc::downgrade(self);
                        move |result: Result<(PathBuf, Option<gdk::Texture>), String>| {
                            if let Some(app) = weak.upgrade() {
                                match result.and_then(|(path, image)| {
                                    media::copy_to_clipboard(&window, &path, image.as_ref())
                                }) {
                                    Ok(()) => app.notify("Copied to clipboard"),
                                    Err(error) => app.notify_error(&error),
                                }
                            }
                        }
                    },
                );
            }
            media::Request::Open(attachment, preview) => {
                let Some(window) = self.window.upgrade() else {
                    return;
                };
                let handler = self.attachment_handler.clone();
                let kind = media::Kind::of(&attachment);
                let edge = media::viewer_image_edge(&window);
                let cached = {
                    let mut state = self.state.borrow_mut();
                    state
                        .viewer_images
                        .iter()
                        .position(|(guid, size, _)| guid == &attachment.guid && *size >= edge)
                        .and_then(|index| state.viewer_images.remove(index))
                        .map(|entry| {
                            let image = entry.2.clone();
                            state.viewer_images.push_back(entry);
                            image
                        })
                };
                let viewer = media::open_viewer(
                    &window,
                    &attachment,
                    cached.as_ref().or(preview.as_ref()),
                    &handler,
                );
                if cached.is_some() {
                    return;
                }
                let image_preview = preview.is_some() && kind == media::Kind::Image;
                let opening = Instant::now();
                let media_file = attachment.clone();
                let weak = Rc::downgrade(self);
                let session = self.state.borrow().session;
                background(
                    move || {
                        let path = {
                            let _profile = crate::profile::span("viewer-read-worker");
                            source.media(&media_file, true)?
                        };
                        let texture = if kind == media::Kind::Image {
                            let _profile = crate::profile::span("viewer-decode-worker");
                            Some(media::image_texture(&path, Some(edge)).map_err(|_| {
                                "This image format could not be opened.".to_string()
                            })?)
                        } else {
                            None
                        };
                        Ok((path, texture))
                    },
                    move |result| {
                        if let (Some(app), Ok((_, Some(texture)))) = (weak.upgrade(), &result) {
                            let mut state = app.state.borrow_mut();
                            if state.session == session {
                                let images = &mut state.viewer_images;
                                images.retain(|(guid, _, _)| guid != &attachment.guid);
                                if images.len() >= 4 {
                                    images.pop_front();
                                }
                                images.push_back((attachment.guid.clone(), edge, texture.clone()));
                            }
                        }
                        let show = move || {
                            let Some(viewer) = viewer.upgrade() else {
                                return;
                            };
                            match result {
                                Ok((path, texture)) => media::show_viewer(
                                    &viewer,
                                    &attachment,
                                    &path,
                                    texture,
                                    &handler,
                                ),
                                Err(error) if !image_preview => {
                                    media::show_viewer_error(&viewer, &error)
                                }
                                Err(_) => {}
                            }
                        };
                        // Keep the small preview texture in place while the dialog animates.
                        let remaining =
                            Duration::from_millis(320).saturating_sub(opening.elapsed());
                        if image_preview && !remaining.is_zero() {
                            glib::timeout_add_local_once(remaining, show);
                        } else {
                            show();
                        }
                    },
                );
            }
            media::Request::Link(url, target) => {
                let host = url.host_str().unwrap_or_default().to_owned();
                preview_background(
                    move || Ok(source.link(&url)),
                    move |result: Result<links::Preview, String>| {
                        if let Ok(preview) = result {
                            target.finish(&host, preview);
                        }
                    },
                );
            }
            media::Request::Save(_) => unreachable!(),
        }
    }

    fn download(self: &Rc<Self>, attachment: Attachment) {
        let (Some(window), Some(server)) =
            (self.window.upgrade(), self.state.borrow().server.clone())
        else {
            return;
        };
        let dialog = gtk::FileDialog::builder().title("Save Attachment").build();
        let basename = std::path::Path::new(&attachment.name)
            .file_name()
            .and_then(|part| part.to_str())
            .unwrap_or("Attachment");
        dialog.set_initial_name(Some(basename));
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let Ok(file) = dialog.save_future(Some(&window)).await else {
                return;
            };
            let Some(path) = file.path() else {
                return;
            };
            background(
                move || server.download(&attachment.guid, &path),
                move |result| {
                    if let Some(app) = weak.upgrade() {
                        match result {
                            Ok(()) => app.notify("Attachment saved"),
                            Err(error) => app.notify_error(&error),
                        }
                    }
                },
            );
        });
    }
}
