//! R help in the system browser.
//!
//! Ark serves help pages only to a frontend that has opened its `positron.help`
//! comm (see `crates/ark/src/help/r_help.rs` in posit-dev/ark); without it, `?topic`
//! logs "Is the help comm open?" and shows nothing. With it, Ark sends
//! `{"method": "show_help", "params": {"content": "<url>", "kind": "url"}}` for
//! each help request, from the editor and from an attached console alike, and the
//! URL points at Ark's local help server.

use collections::HashMap;
use gpui::{App, AppContext as _, Context, Entity, EntityId, Subscription, WeakEntity};
use jupyter_protocol::{CommId, CommOpen, JupyterMessage, JupyterMessageContent};
use repl::{Kernel, Session};
use serde_json::{Map, Value};
use util::ResultExt as _;

use crate::{SessionTracker, TrackerEvent};

const HELP_COMM_TARGET: &str = "positron.help";

/// Keeps a help comm open on every running session the tracker has followed.
/// Comms stay open when the active editor changes, so a console attached to an
/// earlier session still gets its help.
pub(crate) struct HelpComms {
    tracker: Entity<SessionTracker>,
    /// Comm id per session; an entry is dropped when its kernel stops running,
    /// so a restarted kernel gets a new comm.
    comms: HashMap<EntityId, (WeakEntity<Session>, String)>,
    _subscription: Subscription,
}

impl HelpComms {
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
        let comm_id = uuid::Uuid::new_v4().to_string();
        let open = CommOpen {
            comm_id: CommId(comm_id.clone()),
            target_name: HELP_COMM_TARGET.to_string(),
            data: Map::new(),
            target_module: None,
        };
        // A kernel without the target (IJulia, ipykernel) closes the comm; the
        // stale entry is harmless because nothing arrives on it.
        if session
            .update(cx, |session, cx| session.send(open.into(), cx))
            .log_err()
            .is_some()
        {
            self.comms
                .insert(session.entity_id(), (session.downgrade(), comm_id));
        }
    }

    fn handle_kernel_message(&self, message: &JupyterMessage, cx: &App) {
        let JupyterMessageContent::CommMsg(comm_msg) = &message.content else {
            return;
        };
        if !self
            .comms
            .values()
            .any(|(_, comm_id)| *comm_id == comm_msg.comm_id.0)
        {
            return;
        }
        if let Some(url) = help_url(&comm_msg.data) {
            cx.open_url(url);
        }
    }
}

/// The URL of a `show_help` event. Only http(s) URLs are returned, so a kernel
/// cannot make Zed open a local file or another app's URL scheme.
fn help_url(data: &Map<String, Value>) -> Option<&str> {
    if data.get("method")?.as_str()? != "show_help" {
        return None;
    }
    let params = data.get("params")?;
    if params.get("kind")?.as_str()? != "url" {
        return None;
    }
    let url = params.get("content")?.as_str()?;
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
    fn test_help_url_reads_show_help_events() {
        // Shape recorded from Ark 0.1.252 after `?qf`.
        let event = data(json!({
            "method": "show_help",
            "params": {
                "content": "http://127.0.0.1:64113/library/stats/html/Fdist.html",
                "kind": "url",
                "focus": true
            }
        }));
        assert_eq!(
            help_url(&event),
            Some("http://127.0.0.1:64113/library/stats/html/Fdist.html")
        );

        let html = data(json!({
            "method": "show_help",
            "params": { "content": "<p>x</p>", "kind": "html", "focus": true }
        }));
        assert_eq!(help_url(&html), None);

        let file = data(json!({
            "method": "show_help",
            "params": { "content": "file:///etc/passwd", "kind": "url", "focus": true }
        }));
        assert_eq!(help_url(&file), None);

        let other = data(json!({ "method": "refresh", "params": {} }));
        assert_eq!(help_url(&other), None);
    }
}
