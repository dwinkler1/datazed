//! Ark emits each new plot as `display_data` carrying `image/png` plus a
//! `transient.display_id`, and re-renders of the same plot (e.g. after
//! `lines()`) as `update_display_data` with that id.

use std::sync::Arc;

use base64::{
    Engine as _, alphabet,
    engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig},
};
use gpui::{
    App, ClipboardItem, Context, Entity, EventEmitter, FocusHandle, Focusable, Image, ImageFormat,
    ObjectFit, StyledImage as _, TaskExt as _, Window, img,
};
use jupyter_protocol::{JupyterMessage, JupyterMessageContent, MediaType};
use ui::{IconButton, IconName, Tooltip, prelude::*};
use util::ResultExt as _;
use workspace::{
    SplitDirection, Workspace,
    item::{Item, ItemEvent},
};

use crate::{SessionTracker, TrackerEvent};

const MAX_PLOTS: usize = 50;

const BASE64_ANY_PADDING: GeneralPurpose = GeneralPurpose::new(
    &alphabet::STANDARD,
    GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

struct Plot {
    display_id: Option<String>,
    image: Arc<Image>,
    /// Whether the image was drawn, so its atlas texture exists to free.
    shown: bool,
}

pub struct PlotView {
    focus_handle: FocusHandle,
    plots: Vec<Plot>,
    current: usize,
}

impl PlotView {
    fn new(tracker: &Entity<SessionTracker>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.subscribe_in(tracker, window, |this, _tracker, event, window, cx| {
            if let TrackerEvent::KernelMessage(message) = event {
                this.push(message, window, cx);
            }
        })
        .detach();
        cx.on_release_in(window, |this, window, cx| {
            for plot in this.plots.drain(..) {
                release_image(plot, &[], window, cx);
            }
        })
        .detach();
        // on_release_in is skipped when the window has already closed; the
        // window's atlas goes with it, but the decoded images stay in the
        // app-wide asset cache unless removed here.
        cx.on_release(|this, cx| {
            for plot in this.plots.drain(..) {
                plot.image.remove_asset(cx);
            }
        })
        .detach();
        Self {
            focus_handle: cx.focus_handle(),
            plots: Vec::new(),
            current: 0,
        }
    }

    fn push(&mut self, message: &JupyterMessage, window: &mut Window, cx: &mut Context<Self>) {
        let Some(payload) = PlotPayload::from_message(message) else {
            return;
        };
        let Some(image) = decode_png(payload.png_base64) else {
            return;
        };

        let existing = payload.display_id.and_then(|display_id| {
            self.plots
                .iter()
                .position(|plot| plot.display_id.as_deref() == Some(display_id))
        });
        match existing {
            Some(index) => {
                let new_plot = Plot {
                    display_id: self.plots[index].display_id.clone(),
                    image,
                    shown: false,
                };
                let replaced = std::mem::replace(&mut self.plots[index], new_plot);
                release_image(replaced, &self.plots, window, cx);
                self.current = index;
            }
            None => {
                self.plots.push(Plot {
                    display_id: payload.display_id.map(str::to_string),
                    image,
                    shown: false,
                });
                if self.plots.len() > MAX_PLOTS {
                    let evicted = self.plots.remove(0);
                    release_image(evicted, &self.plots, window, cx);
                }
                self.current = self.plots.len() - 1;
            }
        }
        cx.notify();
    }

    #[cfg(test)]
    pub(crate) fn plot_count(&self) -> usize {
        self.plots.len()
    }

    fn step(&mut self, forward: bool, cx: &mut Context<Self>) {
        if forward {
            self.current = (self.current + 1).min(self.plots.len().saturating_sub(1));
        } else {
            self.current = self.current.saturating_sub(1);
        }
        cx.notify();
    }

    fn current_image(&self) -> Option<Arc<Image>> {
        self.plots.get(self.current).map(|plot| plot.image.clone())
    }

    fn copy_current(&mut self, cx: &mut Context<Self>) {
        if let Some(image) = self.current_image() {
            cx.write_to_clipboard(ClipboardItem::new_image(&image));
        }
    }

    fn save_current(&mut self, cx: &mut Context<Self>) {
        let Some(image) = self.current_image() else {
            return;
        };
        let path_prompt = cx.prompt_for_new_path(util::paths::home_dir(), Some("plot.png"));
        cx.background_spawn(async move {
            let Some(path) = path_prompt.await?? else {
                return anyhow::Ok(());
            };
            std::fs::write(&path, image.bytes())?;
            anyhow::Ok(())
        })
        .detach_and_log_err(cx);
    }
}

pub(crate) fn open_plot_view(
    workspace: &mut Workspace,
    tracker: &Entity<SessionTracker>,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let existing = workspace.items_of_type::<PlotView>(cx).next();
    if let Some(existing) = existing {
        workspace.activate_item(&existing, true, true, window, cx);
        return;
    }
    let view = cx.new(|cx| PlotView::new(tracker, window, cx));
    workspace.split_item(SplitDirection::Right, Box::new(view), window, cx);
}

pub(crate) fn open_on_first_plot(
    tracker: &Entity<SessionTracker>,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    cx.subscribe_in(tracker, window, |workspace, tracker, event, window, cx| {
        let TrackerEvent::KernelMessage(message) = event else {
            return;
        };
        if PlotPayload::from_message(message).is_none()
            || workspace.items_of_type::<PlotView>(cx).next().is_some()
        {
            return;
        }
        let view = cx.new(|cx| {
            let mut view = PlotView::new(tracker, window, cx);
            view.push(message, window, cx);
            view
        });
        let previous_focus = window.focused(cx);
        let active_pane = workspace.active_pane().clone();
        let plot_pane = workspace.split_pane(active_pane, SplitDirection::Right, window, cx);
        workspace.add_item(plot_pane, Box::new(view), None, false, false, window, cx);
        if let Some(previous_focus) = previous_focus {
            window.focus(&previous_focus, cx);
        }
    })
    .detach();
}

#[derive(Debug, PartialEq)]
struct PlotPayload<'a> {
    png_base64: &'a str,
    display_id: Option<&'a str>,
}

impl<'a> PlotPayload<'a> {
    fn from_message(message: &'a JupyterMessage) -> Option<Self> {
        let (data, display_id) = match &message.content {
            JupyterMessageContent::DisplayData(display) => (
                &display.data,
                display
                    .transient
                    .as_ref()
                    .and_then(|transient| transient.display_id.as_deref()),
            ),
            JupyterMessageContent::UpdateDisplayData(update) => {
                (&update.data, update.transient.display_id.as_deref())
            }
            _ => return None,
        };
        let png_base64 = data.content.iter().find_map(|media| match media {
            MediaType::Png(png) => Some(png.as_str()),
            _ => None,
        })?;
        Some(Self {
            png_base64,
            display_id,
        })
    }
}

fn decode_png(base64_data: &str) -> Option<Arc<Image>> {
    let filtered: String = base64_data
        .chars()
        .filter(|character| !character.is_ascii_whitespace())
        .collect();
    let bytes = BASE64_ANY_PADDING.decode(filtered).log_err()?;
    Some(Arc::new(Image::from_bytes(ImageFormat::Png, bytes)))
}

/// Frees a plot's decoded image, unless a plot still kept has the same bytes
/// (gpui caches images by content). Only a drawn image has a texture to drop;
/// asking for one would decode the image first.
fn release_image(plot: Plot, kept: &[Plot], window: &mut Window, cx: &mut App) {
    if kept.iter().any(|other| other.image.id() == plot.image.id()) {
        return;
    }
    if plot.shown
        && let Some(render_image) = plot.image.clone().get_render_image(window, cx)
    {
        cx.drop_image(render_image, Some(window));
    }
    plot.image.remove_asset(cx);
}

impl EventEmitter<ItemEvent> for PlotView {}

impl Focusable for PlotView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Item for PlotView {
    type Event = ItemEvent;

    fn tab_content_text(&self, _detail: usize, _cx: &App) -> SharedString {
        "Plots".into()
    }

    fn tab_icon(&self, _window: &Window, _cx: &App) -> Option<Icon> {
        Some(Icon::new(IconName::Image))
    }

    fn show_toolbar(&self) -> bool {
        false
    }

    fn to_item_events(event: &Self::Event, f: &mut dyn FnMut(ItemEvent)) {
        f(*event)
    }
}

impl Render for PlotView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(plot) = self.plots.get_mut(self.current) {
            plot.shown = true;
        }
        let count = self.plots.len();
        let has_plot = count > 0;
        let position = if has_plot {
            format!("{} / {}", self.current + 1, count)
        } else {
            String::new()
        };

        let toolbar = h_flex()
            .px_2()
            .py_1()
            .gap_1()
            .border_b_1()
            .border_color(cx.theme().colors().border)
            .child(
                IconButton::new("previous-plot", IconName::ChevronLeft)
                    .icon_size(IconSize::Small)
                    .disabled(self.current == 0)
                    .tooltip(Tooltip::text("Previous plot"))
                    .on_click(cx.listener(|this, _, _, cx| this.step(false, cx))),
            )
            .child(
                IconButton::new("next-plot", IconName::ChevronRight)
                    .icon_size(IconSize::Small)
                    .disabled(self.current + 1 >= count)
                    .tooltip(Tooltip::text("Next plot"))
                    .on_click(cx.listener(|this, _, _, cx| this.step(true, cx))),
            )
            .child(
                Label::new(position)
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            )
            .child(div().flex_1())
            .child(
                IconButton::new("copy-plot", IconName::Copy)
                    .icon_size(IconSize::Small)
                    .disabled(!has_plot)
                    .tooltip(Tooltip::text("Copy image"))
                    .on_click(cx.listener(|this, _, _, cx| this.copy_current(cx))),
            )
            .child(
                IconButton::new("save-plot", IconName::Download)
                    .icon_size(IconSize::Small)
                    .disabled(!has_plot)
                    .tooltip(Tooltip::text("Save as PNG…"))
                    .on_click(cx.listener(|this, _, _, cx| this.save_current(cx))),
            );

        let body = match self.current_image() {
            Some(image) => div()
                .flex_1()
                .min_h_0()
                .p_2()
                .child(img(image).object_fit(ObjectFit::Contain).size_full())
                .into_any_element(),
            None => v_flex()
                .flex_1()
                .justify_center()
                .items_center()
                .child(Label::new("Plots from the REPL session appear here.").color(Color::Muted))
                .into_any_element(),
        };

        v_flex()
            .id("r-plot-view")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(cx.theme().colors().editor_background)
            .child(toolbar)
            .child(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn message(msg_type: &str, content: serde_json::Value) -> JupyterMessage {
        match JupyterMessageContent::from_type_and_content(msg_type, content) {
            Ok(content) => JupyterMessage::new(content, None),
            Err(error) => panic!("invalid {msg_type} fixture: {error}"),
        }
    }

    #[test]
    fn test_plot_payload_from_ark_messages() {
        let display = message(
            "display_data",
            json!({
                "data": { "image/png": "iVBORw0KGgo=", "text/plain": "<plot>" },
                "metadata": { "image/png": { "width": 672, "height": 480 } },
                "transient": { "display_id": "plot-1" }
            }),
        );
        assert_eq!(
            PlotPayload::from_message(&display),
            Some(PlotPayload {
                png_base64: "iVBORw0KGgo=",
                display_id: Some("plot-1"),
            })
        );

        let update = message(
            "update_display_data",
            json!({
                "data": { "image/png": "AAAA" },
                "metadata": {},
                "transient": { "display_id": "plot-1" }
            }),
        );
        assert_eq!(
            PlotPayload::from_message(&update),
            Some(PlotPayload {
                png_base64: "AAAA",
                display_id: Some("plot-1"),
            })
        );

        let text_only = message(
            "display_data",
            json!({ "data": { "text/plain": "[1] 1" }, "metadata": {} }),
        );
        assert_eq!(PlotPayload::from_message(&text_only), None);
    }

    #[test]
    fn test_decode_png_accepts_unpadded_and_wrapped_base64() {
        let image = decode_png("iVBORw0K\nGgo");
        assert_eq!(
            image.map(|image| image.bytes().to_vec()),
            Some(vec![0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'])
        );
    }
}
