//! Protocol (see `crates/amalthea/src/comm/variables_comm.rs` in posit-dev/ark):
//! the frontend opens the comm, Ark answers with one `refresh` event, then sends
//! an `update` event after every execution. Requests carry an `id` field so Ark
//! treats them as RPCs; the reply is a `comm_msg` whose parent header is the
//! request, with `data = {"method": "<Name>Reply", "result": ...}`.

use std::ops::Range;

use collections::HashMap;
use db::kvp::KeyValueStore;
use gpui::{
    Action, App, ClipboardItem, Context, Entity, EventEmitter, FocusHandle, Focusable, Pixels,
    PromptLevel, ScrollStrategy, Subscription, Task, UniformListScrollHandle, WeakEntity, Window,
    px, uniform_list,
};
use jupyter_protocol::{
    CommClose, CommId, CommMsg, CommOpen, ExecutionState, JupyterMessage, JupyterMessageContent,
};
use menu::{SelectNext, SelectPrevious};
use repl::{Kernel, Session};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use settings::SettingsStore;
use terminal_view::terminal_panel::TerminalPanel;
use ui::{IconButton, IconName, ListItem, Tooltip, prelude::*};
use util::ResultExt as _;
use workspace::{
    Toast, Workspace,
    dock::{DockPosition, Panel, PanelEvent},
    notifications::NotificationId,
};

use crate::{
    CollapseSelectedVariable, CopyVariableName, ExpandSelectedVariable, PrintVariableStructure,
    SessionTracker, ToggleVariables, TrackerEvent, find_console, paste_and_run,
};

const VARIABLES_COMM_TARGET: &str = "positron.variables";
const ACTIVATION_PRIORITY: u32 = 40;
const DOCK_POSITION_KEY: &str = "r_variables_panel_dock";

#[derive(Clone, Debug, Deserialize)]
struct Variable {
    access_key: String,
    display_name: String,
    display_value: String,
    display_type: String,
    #[serde(default)]
    has_children: bool,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "method", content = "params")]
enum VariablesEvent {
    #[serde(rename = "refresh")]
    Refresh { variables: Vec<Variable> },
    #[serde(rename = "update")]
    Update {
        assigned: Vec<Variable>,
        removed: Vec<String>,
    },
}

enum Request {
    List,
    Inspect(Vec<String>),
    Clear,
}

enum Expansion {
    Loading,
    Loaded(Vec<Variable>),
}

struct Row {
    path: Vec<String>,
    depth: usize,
    variable: Variable,
}

#[derive(Default)]
struct VariablesModel {
    listing: Option<Vec<Variable>>,
    expansions: HashMap<Vec<String>, Expansion>,
    pending_requests: HashMap<String, Request>,
    clear_error: Option<String>,
}

impl VariablesModel {
    fn apply(&mut self, reply_to: Option<&str>, data: &Map<String, Value>) {
        if let Some(request) = reply_to.and_then(|msg_id| self.pending_requests.remove(msg_id)) {
            self.apply_reply(request, data);
            return;
        }
        match serde_json::from_value::<VariablesEvent>(Value::Object(data.clone())) {
            Ok(VariablesEvent::Refresh { variables }) => self.replace_listing(variables),
            Ok(VariablesEvent::Update { assigned, removed }) => {
                let Some(listing) = &mut self.listing else {
                    return;
                };
                let changed = |key: &String| {
                    removed.contains(key) || assigned.iter().any(|v| &v.access_key == key)
                };
                self.expansions
                    .retain(|path, _| path.first().is_some_and(|key| !changed(key)));
                listing.retain(|variable| !changed(&variable.access_key));
                listing.extend(assigned);
                listing.sort_by(|left, right| left.display_name.cmp(&right.display_name));
            }
            Err(error) => log::debug!("ignoring unrecognized variables comm message: {error}"),
        }
    }

    fn apply_reply(&mut self, request: Request, data: &Map<String, Value>) {
        let result = data.get("result");
        if result.is_none() {
            log::warn!("variables comm request failed: {:?}", data.get("error"));
        }
        match request {
            Request::List => {
                if let Some(variables) =
                    result.and_then(|result| parse_variables(result.get("variables")))
                {
                    self.replace_listing(variables);
                }
            }
            Request::Clear => {
                self.clear_error = match result {
                    Some(_) => None,
                    None => Some(
                        data.get("error")
                            .and_then(|error| error.get("message"))
                            .and_then(Value::as_str)
                            .unwrap_or("The kernel did not remove the objects.")
                            .to_string(),
                    ),
                };
            }
            Request::Inspect(path) => {
                if !matches!(self.expansions.get(&path), Some(Expansion::Loading)) {
                    return;
                }
                match result.and_then(|result| parse_variables(result.get("children"))) {
                    Some(children) => {
                        self.expansions.insert(path, Expansion::Loaded(children));
                    }
                    // Collapse a row whose inspect failed, so the next click retries.
                    None => {
                        self.expansions.remove(&path);
                    }
                }
            }
        }
    }

    fn replace_listing(&mut self, mut variables: Vec<Variable>) {
        variables.sort_by(|left, right| left.display_name.cmp(&right.display_name));
        self.listing = Some(variables);
        self.expansions.clear();
        self.clear_error = None;
    }

    fn toggle(&mut self, path: &[String]) -> Option<Request> {
        if self.expansions.remove(path).is_some() {
            return None;
        }
        self.expansions.insert(path.to_vec(), Expansion::Loading);
        Some(Request::Inspect(path.to_vec()))
    }

    fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        if let Some(listing) = &self.listing {
            self.push_rows(&mut rows, listing, &[], 0);
        }
        rows
    }

    fn push_rows(
        &self,
        rows: &mut Vec<Row>,
        variables: &[Variable],
        parent: &[String],
        depth: usize,
    ) {
        for variable in variables {
            let mut path = parent.to_vec();
            path.push(variable.access_key.clone());
            let children = match self.expansions.get(&path) {
                Some(Expansion::Loaded(children)) => Some(children),
                _ => None,
            };
            rows.push(Row {
                path: path.clone(),
                depth,
                variable: variable.clone(),
            });
            if let Some(children) = children {
                self.push_rows(rows, children, &path, depth + 1);
            }
        }
    }
}

fn saved_dock_position(cx: &App) -> DockPosition {
    let saved = KeyValueStore::global(cx)
        .read_kvp(DOCK_POSITION_KEY)
        .log_err()
        .flatten();
    match saved.as_deref() {
        Some("left") => DockPosition::Left,
        _ => DockPosition::Right,
    }
}

fn kernel_is_busy(session: &Session) -> bool {
    match &session.kernel {
        Kernel::RunningKernel(kernel) => matches!(kernel.execution_state(), ExecutionState::Busy),
        _ => false,
    }
}

fn parse_variables(value: Option<&Value>) -> Option<Vec<Variable>> {
    serde_json::from_value(value?.clone()).log_err()
}

/// Backquotes a name that R would not parse as a symbol, such as `my var`.
fn r_symbol(name: &str) -> String {
    const RESERVED: [&str; 19] = [
        "if",
        "else",
        "repeat",
        "while",
        "function",
        "for",
        "in",
        "next",
        "break",
        "TRUE",
        "FALSE",
        "NULL",
        "Inf",
        "NaN",
        "NA",
        "NA_integer_",
        "NA_real_",
        "NA_character_",
        "NA_complex_",
    ];
    let mut chars = name.chars();
    let starts_well = match (chars.next(), chars.next()) {
        (Some('.'), second) => !second.is_some_and(|c| c.is_ascii_digit()),
        (Some(first), _) => first.is_alphabetic(),
        (None, _) => false,
    };
    let syntactic = starts_well
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '.' || c == '_')
        && !RESERVED.contains(&name);
    if syntactic {
        name.to_string()
    } else {
        format!("`{}`", name.replace('\\', "\\\\").replace('`', "\\`"))
    }
}

enum Comm {
    Unopened,
    Open {
        session: WeakEntity<Session>,
        comm_id: String,
        /// Whether the kernel has sent anything on the comm yet.
        answered: bool,
        /// Whether `OPEN_TIMEOUT` passed without an answer.
        timed_out: bool,
    },
    ClosedByKernel,
    /// The kernel closed the comm before answering: it has no variables comm.
    Unsupported,
}

/// How long an idle kernel may stay silent after `comm_open` before the panel
/// says it has no variables comm; a later answer still opens the panel.
const OPEN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

pub struct VariablesPanel {
    focus_handle: FocusHandle,
    tracker: Entity<SessionTracker>,
    workspace: WeakEntity<Workspace>,
    position: DockPosition,
    comm: Comm,
    model: VariablesModel,
    rows: Vec<Row>,
    /// The path of the selected row; it stays selected while rows above it
    /// change, and selects nothing once it is gone.
    selected: Option<Vec<String>>,
    scroll_handle: UniformListScrollHandle,
    _open_timeout: Option<Task<()>>,
    _subscription: Subscription,
}

impl VariablesPanel {
    pub(crate) fn new(
        tracker: Entity<SessionTracker>,
        workspace: WeakEntity<Workspace>,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscription = cx.subscribe(&tracker, Self::on_tracker_event);
        Self {
            focus_handle: cx.focus_handle(),
            tracker,
            workspace,
            position: saved_dock_position(cx),
            comm: Comm::Unopened,
            model: VariablesModel::default(),
            rows: Vec::new(),
            selected: None,
            scroll_handle: UniformListScrollHandle::new(),
            _open_timeout: None,
            _subscription: subscription,
        }
    }

    #[cfg(test)]
    pub(crate) fn tracker(&self) -> &Entity<SessionTracker> {
        &self.tracker
    }

    fn on_tracker_event(
        &mut self,
        _tracker: Entity<SessionTracker>,
        event: &TrackerEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            TrackerEvent::SessionChanged => {
                self.close_comm(cx);
                self.open_comm(cx);
            }
            TrackerEvent::SessionUpdated => {
                if self.tracker.read(cx).running_session(cx).is_some() {
                    self.open_comm(cx);
                } else if !matches!(self.comm, Comm::Unopened) {
                    self.comm = Comm::Unopened;
                    self.model = VariablesModel::default();
                    cx.notify();
                }
            }
            TrackerEvent::KernelMessage(message) => self.handle_kernel_message(message, cx),
        }
    }

    fn open_comm(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.comm, Comm::Unopened) {
            return;
        }
        let Some(session) = self.tracker.read(cx).running_session(cx).cloned() else {
            return;
        };
        let comm_id = uuid::Uuid::new_v4().to_string();
        let open = CommOpen {
            comm_id: CommId(comm_id.clone()),
            target_name: VARIABLES_COMM_TARGET.to_string(),
            data: Map::new(),
            target_module: None,
        };
        // Stay unopened if the kernel did not take the request, so the next
        // session update tries again.
        let sent = session.update(cx, |session, cx| session.send(open.into(), cx));
        if sent.log_err().is_none() {
            return;
        }
        self.comm = Comm::Open {
            session: session.downgrade(),
            comm_id,
            answered: false,
            timed_out: false,
        };
        self._open_timeout = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(OPEN_TIMEOUT).await;
                let still_waiting = this.update(cx, |this, cx| {
                    let Comm::Open {
                        session,
                        answered: false,
                        timed_out,
                        ..
                    } = &mut this.comm
                    else {
                        return false;
                    };
                    if session
                        .upgrade()
                        .is_some_and(|session| kernel_is_busy(session.read(cx)))
                    {
                        return true;
                    }
                    *timed_out = true;
                    cx.notify();
                    false
                });
                if !still_waiting.unwrap_or(false) {
                    break;
                }
            }
        }));
        cx.notify();
    }

    fn close_comm(&mut self, cx: &mut Context<Self>) {
        let comm = std::mem::replace(&mut self.comm, Comm::Unopened);
        self.model = VariablesModel::default();
        cx.notify();
        let Comm::Open {
            session, comm_id, ..
        } = comm
        else {
            return;
        };
        let Some(session) = session.upgrade() else {
            return;
        };
        let close = CommClose {
            comm_id: CommId(comm_id),
            data: Map::new(),
        };
        session
            .update(cx, |session, cx| session.send(close.into(), cx))
            .log_err();
    }

    fn handle_kernel_message(&mut self, message: &JupyterMessage, cx: &mut Context<Self>) {
        let Comm::Open {
            comm_id, answered, ..
        } = &mut self.comm
        else {
            return;
        };
        match &message.content {
            JupyterMessageContent::CommClose(close) if &close.comm_id.0 == comm_id => {
                // A kernel without the comm target closes it without answering.
                self.comm = if *answered {
                    Comm::ClosedByKernel
                } else {
                    Comm::Unsupported
                };
                self.model = VariablesModel::default();
            }
            JupyterMessageContent::CommMsg(comm_msg) if &comm_msg.comm_id.0 == comm_id => {
                *answered = true;
                let reply_to = message
                    .parent_header
                    .as_ref()
                    .map(|header| header.msg_id.as_str());
                self.model.apply(reply_to, &comm_msg.data);
            }
            _ => return,
        }
        cx.notify();
    }

    /// `params` is `None` for unit requests such as `list`, which Ark's serde
    /// enums reject when a `params` field is present.
    fn send_request(
        &mut self,
        method: &str,
        params: Option<Value>,
        request: Option<Request>,
        cx: &mut Context<Self>,
    ) {
        let Comm::Open {
            session, comm_id, ..
        } = &self.comm
        else {
            return;
        };
        let Some(session) = session.upgrade() else {
            return;
        };
        let mut data = Map::new();
        data.insert("jsonrpc".into(), json!("2.0"));
        data.insert("method".into(), json!(method));
        if let Some(params) = params {
            data.insert("params".into(), params);
        }
        let mut message: JupyterMessage = CommMsg {
            comm_id: CommId(comm_id.clone()),
            data,
        }
        .into();
        let msg_id = message.header.msg_id.clone();
        if let JupyterMessageContent::CommMsg(comm_msg) = &mut message.content {
            comm_msg.data.insert("id".into(), json!(msg_id));
        }
        let sent = session.update(cx, |session, cx| session.send(message, cx));
        match (sent.log_err(), request) {
            (Some(()), Some(request)) => {
                self.model.pending_requests.insert(msg_id, request);
            }
            // Collapse the row again, so the next click retries.
            (None, Some(Request::Inspect(path))) => {
                self.model.expansions.remove(&path);
            }
            _ => {}
        }
    }

    fn toggle_expanded(&mut self, path: Vec<String>, cx: &mut Context<Self>) {
        if let Some(request) = self.model.toggle(&path) {
            self.send_request("inspect", Some(json!({ "path": path })), Some(request), cx);
        }
        cx.notify();
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        match self.comm {
            Comm::Open { answered: true, .. } => {
                self.send_request("list", None, Some(Request::List), cx)
            }
            Comm::Open {
                answered: false, ..
            }
            | Comm::ClosedByKernel
            | Comm::Unsupported => {
                self.close_comm(cx);
                self.open_comm(cx);
            }
            Comm::Unopened => self.open_comm(cx),
        }
    }

    fn confirm_clear(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Comm::Open { comm_id, .. } = &self.comm else {
            return;
        };
        let asked_comm_id = comm_id.clone();
        let answer = window.prompt(
            PromptLevel::Warning,
            "Remove all objects from the global environment?",
            None,
            &["Remove All", "Cancel"],
            cx,
        );
        cx.spawn(async move |this, cx| {
            if answer.await == Ok(0) {
                this.update(cx, |this, cx| {
                    // The session may have changed while the prompt was open.
                    if !matches!(&this.comm, Comm::Open { comm_id, .. } if *comm_id == asked_comm_id) {
                        return;
                    }
                    this.send_request(
                        "clear",
                        Some(json!({ "include_hidden_objects": false })),
                        Some(Request::Clear),
                        cx,
                    );
                })
                .ok();
            }
        })
        .detach();
    }

    /// The selected row and its index among the rows the model shows now,
    /// which may differ from `self.rows` until the next render.
    fn selected_row(&self) -> Option<(usize, Row)> {
        let selected = self.selected.as_ref()?;
        self.model
            .rows()
            .into_iter()
            .enumerate()
            .find(|(_, row)| &row.path == selected)
    }

    fn select(&mut self, path: Vec<String>, index: usize, cx: &mut Context<Self>) {
        self.selected = Some(path);
        self.scroll_handle
            .scroll_to_item(index, ScrollStrategy::Nearest);
        cx.notify();
    }

    fn move_selection(&mut self, forward: bool, cx: &mut Context<Self>) {
        let mut rows = self.model.rows();
        let Some(last) = rows.len().checked_sub(1) else {
            return;
        };
        let index = match (self.selected_row(), forward) {
            (Some((index, _)), true) => (index + 1).min(last),
            (Some((index, _)), false) => index.saturating_sub(1),
            (None, true) => 0,
            (None, false) => last,
        };
        self.select(rows.swap_remove(index).path, index, cx);
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        self.move_selection(true, cx);
    }

    fn select_previous(&mut self, _: &SelectPrevious, _: &mut Window, cx: &mut Context<Self>) {
        self.move_selection(false, cx);
    }

    fn expand_selected(
        &mut self,
        _: &ExpandSelectedVariable,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((index, row)) = self.selected_row() else {
            return;
        };
        if !row.variable.has_children {
            return;
        }
        match self.model.expansions.get(&row.path) {
            None => self.toggle_expanded(row.path, cx),
            Some(Expansion::Loaded(children)) if !children.is_empty() => {
                let mut first_child = row.path;
                first_child.push(children[0].access_key.clone());
                self.select(first_child, index + 1, cx);
            }
            Some(_) => {}
        }
    }

    fn collapse_selected(
        &mut self,
        _: &CollapseSelectedVariable,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((_, mut row)) = self.selected_row() else {
            return;
        };
        if self.model.expansions.contains_key(&row.path) {
            self.toggle_expanded(row.path, cx);
            return;
        }
        if row.path.len() > 1 {
            row.path.pop();
            if let Some(index) = self.model.rows().iter().position(|r| r.path == row.path) {
                self.select(row.path, index, cx);
            }
        }
    }

    fn copy_selected_name(&mut self, _: &CopyVariableName, _: &mut Window, cx: &mut Context<Self>) {
        if let Some((_, row)) = self.selected_row() {
            cx.write_to_clipboard(ClipboardItem::new_string(row.variable.display_name));
        }
    }

    fn print_selected_structure(
        &mut self,
        _: &PrintVariableStructure,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        struct CannotPrint;

        let Some((_, row)) = self.selected_row() else {
            return;
        };
        if let Err(message) = self.print_structure(&row, window, cx) {
            self.workspace
                .update(cx, |workspace, cx| {
                    workspace.show_toast(
                        Toast::new(NotificationId::unique::<CannotPrint>(), message).autohide(),
                        cx,
                    )
                })
                .ok();
        }
    }

    /// Pastes `str(name)` (R) or `dump(name; maxdepth=1)` (Julia) into the
    /// console that OpenConsole opened on this session, so the output appears
    /// there and the object is read by the same kernel. Only top-level objects
    /// are printed: a nested row's access keys are not an expression.
    fn print_structure(
        &self,
        row: &Row,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), &'static str> {
        if row.path.len() > 1 {
            return Err("Only top-level objects can be printed in the console.");
        }
        let session = self
            .tracker
            .read(cx)
            .running_session(cx)
            .ok_or("Run some code to start a kernel session.")?
            .read(cx);
        let name = &row.variable.display_name;
        let code = match session
            .kernel_specification
            .language()
            .to_lowercase()
            .as_str()
        {
            "r" => format!("str({})", r_symbol(name)),
            "julia" => format!("dump({name}; maxdepth=1)"),
            _ => return Err("Printing an object's structure works with R and Julia kernels."),
        };
        let Kernel::RunningKernel(kernel) = &session.kernel else {
            return Err("Run some code to start a kernel session.");
        };
        let connection_file = kernel
            .connection_file()
            .ok_or("This kernel has no connection file for a console.")?
            .to_path_buf();
        let workspace = self.workspace.upgrade().ok_or("The workspace is closed.")?;
        workspace.update(cx, |workspace, cx| {
            let (pane, index, terminal_view) = find_console(workspace, &connection_file, cx)
                .ok_or("Open the REPL console to print an object's structure there.")?;
            paste_and_run(&terminal_view, &code, cx);
            // Show the console without taking focus from the panel, so the
            // keys keep moving through the variables.
            pane.update(cx, |pane, cx| {
                pane.activate_item(index, false, false, window, cx)
            });
            workspace.open_panel::<TerminalPanel>(window, cx);
            Ok(())
        })
    }

    /// A click selects the row, copies its name and prints its structure in
    /// the console when one is open; the disclosure arrow expands it.
    fn click_row(&mut self, path: Vec<String>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.model.rows().iter().position(|row| row.path == path) else {
            return;
        };
        window.focus(&self.focus_handle, cx);
        self.select(path, index, cx);
        self.copy_selected_name(&CopyVariableName, window, cx);
        if let Some((_, row)) = self.selected_row() {
            self.print_structure(&row, window, cx).ok();
        }
    }

    fn render_row(&self, index: usize, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let row = &self.rows[index];
        let variable = &row.variable;
        let is_expanded = self.model.expansions.contains_key(&row.path);
        let is_selected = self.selected.as_ref() == Some(&row.path);
        let toggle_path = row.path.clone();
        let click_path = row.path.clone();

        // ListItem draws the disclosure 1rem left of its content, so every row
        // is indented one extra level to keep top-level arrows inside the panel.
        ListItem::new(index)
            .indent_level(row.depth + 1)
            .indent_step_size(window.rem_size() + px(4.))
            .always_show_disclosure_icon(true)
            .toggle_state(is_selected)
            .toggle(variable.has_children.then_some(is_expanded))
            .on_toggle(cx.listener(move |this, _, _, cx| {
                this.toggle_expanded(toggle_path.clone(), cx);
            }))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.click_row(click_path.clone(), window, cx);
            }))
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .child(
                        div()
                            .flex_none()
                            .child(Label::new(variable.display_name.clone())),
                    )
                    .child(
                        div().flex_none().child(
                            Label::new(variable.display_type.clone())
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        ),
                    )
                    .child(
                        div().flex_1().min_w_0().child(
                            Label::new(variable.display_value.clone())
                                .color(Color::Muted)
                                .truncate(),
                        ),
                    ),
            )
            .into_any_element()
    }

    fn empty_state_message(&self, cx: &App) -> &'static str {
        if self.tracker.read(cx).session().is_none() {
            return "Run some code to start a kernel session.";
        }
        match self.comm {
            Comm::Unopened => "Waiting for the kernel…",
            Comm::Unsupported
            | Comm::Open {
                answered: false,
                timed_out: true,
                ..
            } => {
                "This kernel has no variables view. It needs Ark, Positron's Python kernel or DataZedVariables in IJulia."
            }
            Comm::ClosedByKernel => {
                "The kernel closed the variables connection. Refresh to reconnect."
            }
            Comm::Open { .. } if self.model.listing.is_none() => "Loading…",
            Comm::Open { .. } => "The global environment is empty.",
        }
    }
}

impl EventEmitter<PanelEvent> for VariablesPanel {}

impl Focusable for VariablesPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for VariablesPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.rows = self.model.rows();
        let is_open = matches!(self.comm, Comm::Open { .. });

        let toolbar = h_flex()
            .px_2()
            .py_1()
            .gap_1()
            .justify_between()
            .border_b_1()
            .border_color(cx.theme().colors().border)
            .child(Label::new("Global Environment").size(LabelSize::Small))
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        IconButton::new("refresh-variables", IconName::RotateCw)
                            .icon_size(IconSize::Small)
                            .disabled(self.tracker.read(cx).session().is_none())
                            .tooltip(Tooltip::text("Refresh"))
                            .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
                    )
                    .child(
                        IconButton::new("clear-variables", IconName::Eraser)
                            .icon_size(IconSize::Small)
                            .disabled(!is_open)
                            .tooltip(Tooltip::text("Remove all objects"))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.confirm_clear(window, cx)),
                            ),
                    ),
            );

        let body = if self.rows.is_empty() {
            v_flex()
                .size_full()
                .p_4()
                .justify_center()
                .items_center()
                .child(Label::new(self.empty_state_message(cx)).color(Color::Muted))
                .into_any_element()
        } else {
            uniform_list(
                "r-variables",
                self.rows.len(),
                cx.processor(|this, range: Range<usize>, window, cx| {
                    range
                        .map(|index| this.render_row(index, window, cx))
                        .collect::<Vec<_>>()
                }),
            )
            .size_full()
            .track_scroll(&self.scroll_handle)
            .into_any_element()
        };

        v_flex()
            .id("r-variables-panel")
            .key_context("VariablesPanel")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_previous))
            .on_action(cx.listener(Self::expand_selected))
            .on_action(cx.listener(Self::collapse_selected))
            .on_action(cx.listener(Self::copy_selected_name))
            .on_action(cx.listener(Self::print_selected_structure))
            .size_full()
            .bg(cx.theme().colors().panel_background)
            .child(toolbar)
            .children(self.model.clear_error.clone().map(|error| {
                div()
                    .px_2()
                    .py_1()
                    .child(Label::new(error).size(LabelSize::Small).color(Color::Error))
            }))
            .child(body)
    }
}

impl Panel for VariablesPanel {
    fn persistent_name() -> &'static str {
        "RVariablesPanel"
    }

    fn panel_key() -> &'static str {
        "r_variables_panel"
    }

    fn position(&self, _window: &Window, _cx: &App) -> DockPosition {
        self.position
    }

    fn position_is_valid(&self, position: DockPosition) -> bool {
        matches!(position, DockPosition::Left | DockPosition::Right)
    }

    fn set_position(
        &mut self,
        position: DockPosition,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.position = position;
        let saved = match position {
            DockPosition::Left => "left",
            DockPosition::Bottom | DockPosition::Right => "right",
        };
        let kvp = KeyValueStore::global(cx);
        db::write_and_log(cx, move || async move {
            kvp.write_kvp(DOCK_POSITION_KEY.into(), saved.into()).await
        });
        // The workspace re-reads panel positions only when settings change.
        // Zed's own panels store their side in settings; this one signals the
        // same change without writing settings.json, which Zed shares. Every
        // settings observer runs, which also re-sends language server
        // configuration; that happens only when the panel is moved.
        cx.update_global::<SettingsStore, _>(|_, _| {});
        cx.notify();
    }

    fn default_size(&self, _window: &Window, _cx: &App) -> Pixels {
        px(320.)
    }

    fn icon(&self, _window: &Window, _cx: &App) -> Option<IconName> {
        Some(IconName::ListTree)
    }

    fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
        Some("Variables")
    }

    fn toggle_action(&self) -> Box<dyn Action> {
        Box::new(ToggleVariables)
    }

    fn activation_priority(&self) -> u32 {
        ACTIVATION_PRIORITY
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ark_serialized_variable(name: &str, has_children: bool) -> Value {
        json!({
            "access_key": name, "display_name": name, "display_type": "dbl [3]",
            "display_value": "1 2 3", "has_children": has_children, "has_viewer": false,
            "is_truncated": false, "kind": "collection", "length": 3, "size": 80,
            "type_info": "double", "updated_time": 0
        })
    }

    fn data(value: Value) -> Map<String, Value> {
        match value {
            Value::Object(map) => map,
            other => panic!("expected a JSON object, got {other}"),
        }
    }

    fn outline(model: &VariablesModel) -> Vec<(usize, String)> {
        model
            .rows()
            .into_iter()
            .map(|row| (row.depth, row.variable.display_name))
            .collect()
    }

    fn names(depth_and_names: &[(usize, &str)]) -> Vec<(usize, String)> {
        depth_and_names
            .iter()
            .map(|(depth, name)| (*depth, name.to_string()))
            .collect()
    }

    #[test]
    fn test_refresh_then_update() {
        let mut model = VariablesModel::default();
        model.apply(
            Some("execute-1"),
            &data(json!({
                "method": "refresh",
                "params": { "length": 2, "variables": [ark_serialized_variable("b", false), ark_serialized_variable("a", false)], "version": 1 }
            })),
        );
        assert_eq!(outline(&model), names(&[(0, "a"), (0, "b")]));

        model.apply(
            Some("execute-2"),
            &data(json!({
                "method": "update",
                "params": { "assigned": [ark_serialized_variable("c", false)], "removed": ["a"], "unevaluated": [], "version": 2 }
            })),
        );
        assert_eq!(outline(&model), names(&[(0, "b"), (0, "c")]));
    }

    #[test]
    fn test_inspect_reply_is_routed_by_parent_header() {
        let mut model = VariablesModel::default();
        model.apply(
            None,
            &data(json!({ "method": "refresh", "params": { "length": 1, "variables": [ark_serialized_variable("df", true)], "version": 1 } })),
        );
        let Some(request) = model.toggle(&["df".to_string()]) else {
            panic!("expanding a collapsed variable should request its children");
        };
        model.pending_requests.insert("request-1".into(), request);

        model.apply(
            Some("request-1"),
            &data(json!({ "method": "InspectReply", "result": { "children": [ark_serialized_variable("x", false)], "length": 1 } })),
        );
        assert_eq!(outline(&model), names(&[(0, "df"), (1, "x")]));
    }

    #[test]
    fn test_inspect_reply_after_collapse_is_ignored() {
        let mut model = VariablesModel::default();
        model.apply(
            None,
            &data(json!({ "method": "refresh", "params": { "length": 1, "variables": [ark_serialized_variable("df", true)], "version": 1 } })),
        );
        let path = vec!["df".to_string()];
        let Some(request) = model.toggle(&path) else {
            panic!("expanding a collapsed variable should request its children");
        };
        model.pending_requests.insert("request-1".into(), request);
        assert!(model.toggle(&path).is_none());

        model.apply(
            Some("request-1"),
            &data(json!({ "method": "InspectReply", "result": { "children": [ark_serialized_variable("x", false)], "length": 1 } })),
        );
        assert_eq!(outline(&model), names(&[(0, "df")]));
    }

    #[test]
    fn test_list_reply_replaces_listing() {
        let mut model = VariablesModel::default();
        model
            .pending_requests
            .insert("request-1".into(), Request::List);
        model.apply(
            Some("request-1"),
            &data(json!({ "method": "ListReply", "result": { "length": 1, "variables": [ark_serialized_variable("z", false)], "version": 3 } })),
        );
        assert_eq!(outline(&model), names(&[(0, "z")]));
    }

    #[test]
    fn test_refused_clear_is_reported_until_the_next_listing() {
        let mut model = VariablesModel::default();
        model
            .pending_requests
            .insert("request-1".into(), Request::Clear);
        model.apply(
            Some("request-1"),
            &data(json!({ "jsonrpc": "2.0", "id": "request-1", "error": { "code": -32000, "message": "cannot remove" } })),
        );
        assert_eq!(model.clear_error.as_deref(), Some("cannot remove"));

        model.apply(
            None,
            &data(json!({ "method": "refresh", "params": { "length": 0, "variables": [], "version": 2 } })),
        );
        assert_eq!(model.clear_error, None);
    }

    #[test]
    fn test_r_symbol_backquotes_non_syntactic_names() {
        assert_eq!(r_symbol("df"), "df");
        assert_eq!(r_symbol(".hidden_x2"), ".hidden_x2");
        assert_eq!(r_symbol("my var"), "`my var`");
        assert_eq!(r_symbol(".2x"), "`.2x`");
        assert_eq!(r_symbol("2x"), "`2x`");
        assert_eq!(r_symbol("if"), "`if`");
        assert_eq!(r_symbol("a`b"), "`a\\`b`");
    }

    #[gpui::test]
    async fn test_keys_move_expand_and_copy(cx: &mut gpui::TestAppContext) {
        let (workspace, cx) = crate::tests::r_tools_workspace(cx).await;
        let panel = workspace.read_with(cx, |workspace, cx| {
            match workspace.panel::<VariablesPanel>(cx) {
                Some(panel) => panel,
                None => panic!("variables panel was not registered"),
            }
        });
        panel.update(cx, |panel, _| {
            panel.model.apply(
                None,
                &data(json!({ "method": "refresh", "params": { "length": 2, "variables": [ark_serialized_variable("df", true), ark_serialized_variable("y", false)], "version": 1 } })),
            );
            panel.model.expansions.insert(
                vec!["df".into()],
                Expansion::Loaded(vec![serde_json::from_value(ark_serialized_variable("x", false)).unwrap()]),
            );
        });
        let selected = |cx: &mut gpui::VisualTestContext| {
            panel.read_with(cx, |panel, _| panel.selected.clone())
        };
        let path = |keys: &[&str]| Some(keys.iter().map(|key| key.to_string()).collect::<Vec<_>>());

        panel.update_in(cx, |panel, window, cx| {
            panel.select_next(&SelectNext, window, cx);
            panel.expand_selected(&ExpandSelectedVariable, window, cx);
        });
        assert_eq!(selected(cx), path(&["df", "x"]));

        panel.update_in(cx, |panel, window, cx| {
            panel.collapse_selected(&CollapseSelectedVariable, window, cx)
        });
        assert_eq!(selected(cx), path(&["df"]));

        panel.update_in(cx, |panel, window, cx| {
            panel.collapse_selected(&CollapseSelectedVariable, window, cx);
            panel.select_next(&SelectNext, window, cx);
            panel.select_next(&SelectNext, window, cx);
            panel.copy_selected_name(&CopyVariableName, window, cx);
        });
        assert_eq!(selected(cx), path(&["y"]));
        assert!(panel.read_with(cx, |panel, _| panel.model.expansions.is_empty()));
        let copied = cx.read_from_clipboard().and_then(|item| item.text());
        assert_eq!(copied.as_deref(), Some("y"));
    }
}
