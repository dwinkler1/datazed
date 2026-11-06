//! R help, `browseURL()` and HTML files in the system browser.
//!
//! Ark (posit-dev/ark) sends these only to a frontend that has opened its comms:
//! - `positron.help` (`crates/ark/src/help/r_help.rs`): without it, `?topic` logs
//!   "Is the help comm open?" and shows nothing. With it, each help request
//!   arrives as `{"method": "show_help", "params": {"content": "<url>", "kind": "url"}}`
//!   pointing at Ark's local help server.
//! - `positron.ui` (`crates/ark/src/ui/`): `browseURL()` of a web URL arrives as
//!   `show_url` (httpgd's `hgd_browse()` uses this), and of a file path as
//!   `open_with_system` (htmlwidgets such as plotly, once Ark's inline widget
//!   printing is off). Ark's other UI events, such as `busy`, are ignored.
//!
//! Requests from the editor and from an attached console arrive alike.

use std::path::Path;

use collections::HashMap;
use gpui::{App, AppContext as _, Context, Entity, EntityId, Subscription, WeakEntity};
use jupyter_protocol::{CommId, CommOpen, JupyterMessage, JupyterMessageContent};
use repl::{Kernel, Session};
use serde_json::{Map, Value};
use util::ResultExt as _;

use crate::{SessionTracker, TrackerEvent};

const COMM_TARGETS: [&str; 2] = ["positron.help", "positron.ui"];

/// Keeps the help and UI comms open on every running session the tracker has
/// followed. Comms stay open when the active editor changes, so a console
/// attached to an earlier session still gets its help and browser requests.
pub(crate) struct BrowserComms {
    tracker: Entity<SessionTracker>,
    /// Comm ids per session; an entry is dropped when its kernel stops running,
    /// so a restarted kernel gets new comms.
    comms: HashMap<EntityId, (WeakEntity<Session>, Vec<String>)>,
    _subscription: Subscription,
}

impl BrowserComms {
    pub(crate) fn new(tracker: Entity<SessionTracker>, cx: &mut App) -> Entity<Self> {
        cx.new(|cx| {
            let subscription = cx.subscribe(&tracker, Self::on_tracker_event);
            let mut this = Self {
                tracker,
                comms: HashMap::default(),
                _subscription: subscription,
            };
            this.open_for_running_session(cx);
            this
        })
    }

    fn on_tracker_event(
        &mut self,
        _tracker: Entity<SessionTracker>,
        event: &TrackerEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            TrackerEvent::SessionChanged | TrackerEvent::SessionUpdated => {
                self.forget_stopped_sessions(cx);
                self.open_for_running_session(cx);
            }
            TrackerEvent::KernelMessage(message) => self.handle_kernel_message(message, cx),
        }
    }

    fn forget_stopped_sessions(&mut self, cx: &App) {
        self.comms.retain(|_, (session, _)| {
            session
                .upgrade()
                .is_some_and(|session| matches!(session.read(cx).kernel, Kernel::RunningKernel(_)))
        });
    }

    fn open_for_running_session(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.tracker.read(cx).running_session(cx).cloned() else {
            return;
        };
        if self.comms.contains_key(&session.entity_id()) {
            return;
        }
        // A kernel without these targets (IJulia, ipykernel) closes the comms;
        // the stale ids are harmless because nothing arrives on them.
        let mut comm_ids = Vec::new();
        for target in COMM_TARGETS {
            let comm_id = uuid::Uuid::new_v4().to_string();
            let open = CommOpen {
                comm_id: CommId(comm_id.clone()),
                target_name: target.to_string(),
                data: Map::new(),
                target_module: None,
            };
            if session
                .update(cx, |session, cx| session.send(open.into(), cx))
                .log_err()
                .is_some()
            {
                comm_ids.push(comm_id);
            }
        }
        if !comm_ids.is_empty() {
            self.comms
                .insert(session.entity_id(), (session.downgrade(), comm_ids));
        }
    }

    fn handle_kernel_message(&self, message: &JupyterMessage, cx: &App) {
        let JupyterMessageContent::CommMsg(comm_msg) = &message.content else {
            return;
        };
        if !self
            .comms
            .values()
            .any(|(_, comm_ids)| comm_ids.contains(&comm_msg.comm_id.0))
        {
            return;
        }
        match browser_request(&comm_msg.data) {
            Some(BrowserRequest::Url(url)) => cx.open_url(url),
            Some(BrowserRequest::HtmlFile(path)) => cx.open_with_system(path),
            None => {}
        }
    }
}

#[derive(Debug, PartialEq)]
enum BrowserRequest<'a> {
    Url(&'a str),
    HtmlFile(&'a Path),
}

/// What a help or UI comm message asks DataZed to open. Only http(s) URLs and
/// absolute `.html`/`.htm` paths are returned, so a kernel cannot make Zed
/// launch another file type or URL scheme.
fn browser_request(data: &Map<String, Value>) -> Option<BrowserRequest<'_>> {
    let params = data.get("params")?;
    match data.get("method")?.as_str()? {
        "show_help" if params.get("kind")?.as_str()? == "url" => {
            web_url(params.get("content")?.as_str()?).map(BrowserRequest::Url)
        }
        "show_url" => web_url(params.get("url")?.as_str()?).map(BrowserRequest::Url),
        "open_with_system" => {
            let path = Path::new(params.get("path")?.as_str()?);
            let is_html = path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| {
                    extension.eq_ignore_ascii_case("html") || extension.eq_ignore_ascii_case("htm")
                });
            (path.is_absolute() && is_html).then_some(BrowserRequest::HtmlFile(path))
        }
        _ => None,
    }
}

fn web_url(url: &str) -> Option<&str> {
    (url.starts_with("http://") || url.starts_with("https://")).then_some(url)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn data(value: Value) -> Map<String, Value> {
        value.as_object().cloned().unwrap()
    }

    #[test]
    fn test_browser_request_reads_help_and_ui_events() {
        // Shapes recorded from Ark 0.1.252: `?qf`, `httpgd::hgd_browse()`, and
        // a plotly widget with Ark's inline widget printing off.
        let help = data(json!({
            "method": "show_help",
            "params": {
                "content": "http://127.0.0.1:64113/library/stats/html/Fdist.html",
                "kind": "url",
                "focus": true
            }
        }));
        assert_eq!(
            browser_request(&help),
            Some(BrowserRequest::Url(
                "http://127.0.0.1:64113/library/stats/html/Fdist.html"
            ))
        );

        let httpgd = data(json!({
            "method": "show_url",
            "params": { "url": "http://127.0.0.1:52121/live?token=4VFEs9Ck", "source": null }
        }));
        assert_eq!(
            browser_request(&httpgd),
            Some(BrowserRequest::Url(
                "http://127.0.0.1:52121/live?token=4VFEs9Ck"
            ))
        );

        let widget = data(json!({
            "method": "open_with_system",
            "params": { "path": "/private/var/folders/T/RtmpM90QUK/viewhtml48804bc7e700/index.html" }
        }));
        assert_eq!(
            browser_request(&widget),
            Some(BrowserRequest::HtmlFile(Path::new(
                "/private/var/folders/T/RtmpM90QUK/viewhtml48804bc7e700/index.html"
            )))
        );
    }

    #[test]
    fn test_browser_request_refuses_other_targets() {
        let cases = [
            json!({ "method": "show_help", "params": { "content": "<p>x</p>", "kind": "html" } }),
            json!({ "method": "show_help", "params": { "content": "file:///etc/passwd", "kind": "url" } }),
            json!({ "method": "show_url", "params": { "url": "file:///etc/passwd" } }),
            json!({ "method": "open_with_system", "params": { "path": "/Applications/Calculator.app" } }),
            json!({ "method": "open_with_system", "params": { "path": "/tmp/run.command" } }),
            json!({ "method": "open_with_system", "params": { "path": "relative/index.html" } }),
            json!({ "method": "busy", "params": { "busy": true } }),
        ];
        for case in cases {
            assert_eq!(browser_request(&data(case.clone())), None, "{case}");
        }
    }
}
