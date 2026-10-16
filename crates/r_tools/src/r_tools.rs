mod plot_view;

use std::sync::Arc;

use editor::{Editor, SelectionEffects, scroll::Autoscroll};
use gpui::{App, AppContext as _, Context, Entity, EventEmitter, Subscription, Window, actions};
use jupyter_protocol::JupyterMessage;
use repl::{Kernel, KernelMessage, ReplStore, Session, SessionEvent};
use terminal_view::{TerminalView, terminal_panel::TerminalPanel};
use workspace::{Toast, Workspace, dock::Panel as _, notifications::NotificationId};

pub use plot_view::PlotView;

actions!(
    r_tools,
    [
        /// Opens the R plot viewer in a split pane.
        OpenPlots,
        /// Sends the current top-level expression or selection to the active terminal.
        SendToTerminal,
    ]
);

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, window, cx| {
        let Some(window) = window else {
            return;
        };
        let workspace_entity = cx.entity();
        let tracker = cx.new(|cx| SessionTracker::new(&workspace_entity, cx));

        plot_view::open_on_first_plot(&tracker, window, cx);

        workspace.register_action(move |workspace, _: &OpenPlots, window, cx| {
            plot_view::open_plot_view(workspace, &tracker, window, cx);
        });
        workspace.register_action(send_to_terminal);
    })
    .detach();
}

pub struct SessionTracker {
    session: Option<Entity<Session>>,
    _workspace_subscription: Subscription,
    _watch_editor_for_new_session: Option<Subscription>,
    _session_subscriptions: Vec<Subscription>,
}

pub enum TrackerEvent {
    SessionChanged,
    SessionUpdated,
    KernelMessage(Arc<JupyterMessage>),
}

impl EventEmitter<TrackerEvent> for SessionTracker {}

impl SessionTracker {
    fn new(workspace: &Entity<Workspace>, cx: &mut Context<Self>) -> Self {
        let workspace_subscription = cx.subscribe(workspace, |this, workspace, event, cx| {
            if let workspace::Event::ActiveItemChanged = event {
                this.follow_active_editor(&workspace, cx);
            }
        });
        Self {
            session: None,
            _workspace_subscription: workspace_subscription,
            _watch_editor_for_new_session: None,
            _session_subscriptions: Vec::new(),
        }
    }

    pub fn session(&self) -> Option<&Entity<Session>> {
        self.session.as_ref()
    }

    pub fn running_session(&self, cx: &App) -> Option<&Entity<Session>> {
        self.session
            .as_ref()
            .filter(|session| matches!(session.read(cx).kernel, Kernel::RunningKernel(_)))
    }

    fn follow_active_editor(&mut self, workspace: &Entity<Workspace>, cx: &mut Context<Self>) {
        let Some(editor) = workspace.read(cx).active_item_as::<Editor>(cx) else {
            return;
        };
        self._watch_editor_for_new_session = Some(cx.observe(&editor, |this, editor, cx| {
            this.attach_session_for(&editor, cx);
        }));
        self.attach_session_for(&editor, cx);
    }

    fn attach_session_for(&mut self, editor: &Entity<Editor>, cx: &mut Context<Self>) {
        let Some(session) = ReplStore::global(cx)
            .read(cx)
            .get_session(editor.entity_id())
            .cloned()
        else {
            return;
        };
        if self.session.as_ref() == Some(&session)
            || !session
                .read(cx)
                .kernel_specification
                .language()
                .eq_ignore_ascii_case("r")
        {
            return;
        }

        self._session_subscriptions = vec![
            cx.subscribe(&session, |_this, _session, event: &KernelMessage, cx| {
                cx.emit(TrackerEvent::KernelMessage(event.0.clone()));
            }),
            cx.subscribe(&session, |this, _session, event: &SessionEvent, cx| {
                let SessionEvent::Shutdown(_) = event;
                this.session = None;
                cx.emit(TrackerEvent::SessionChanged);
            }),
            cx.observe(&session, |_this, _session, cx| {
                cx.emit(TrackerEvent::SessionUpdated);
            }),
        ];
        // Plots of the followed session go to the plots pane only; a session
        // no longer followed shows its plots inline again.
        if let Some(previous) = self.session.take() {
            previous.update(cx, |previous, _| previous.plots_shown_in_pane = false);
        }
        session.update(cx, |session, _| session.plots_shown_in_pane = true);
        self.session = Some(session);
        cx.emit(TrackerEvent::SessionChanged);
    }
}

fn send_to_terminal(
    workspace: &mut Workspace,
    _: &SendToTerminal,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    struct NoTerminal;

    /// Whether a terminal's foreground process is an R console (R or radian).
    /// Code is pasted only into these, so a shell never runs R code (where
    /// `x > y` would overwrite `y`).
    fn is_r_console(name: &str, argv: &[String]) -> bool {
        const CONSOLES: [&str; 3] = ["r", "rterm", "radian"];
        std::iter::once(name)
            .chain(argv.iter().map(String::as_str))
            .filter_map(|arg| std::path::Path::new(arg).file_name()?.to_str())
            .map(|file_name| file_name.to_ascii_lowercase())
            .any(|file_name| CONSOLES.contains(&file_name.trim_end_matches(".exe")))
    }

    let Some(editor) = workspace.active_item_as::<Editor>(cx) else {
        return;
    };
    let terminal_view = workspace
        .panel::<TerminalPanel>(cx)
        .and_then(|panel| panel.read(cx).pane())
        .and_then(|pane| pane.read(cx).active_item())
        .and_then(|item| item.downcast::<TerminalView>());
    let runs_r = terminal_view.as_ref().is_some_and(|terminal_view| {
        terminal_view
            .read(cx)
            .terminal()
            .read(cx)
            .foreground_process()
            .is_some_and(|(name, argv)| is_r_console(&name, &argv))
    });
    let Some(terminal_view) = terminal_view.filter(|_| runs_r) else {
        workspace.show_toast(
            Toast::new(
                NotificationId::unique::<NoTerminal>(),
                "The active terminal is not running R. Start R or radian there first.",
            )
            .autohide(),
            cx,
        );
        return;
    };

    let picked = editor.update(cx, |editor, cx| {
        let selection = editor
            .selections
            .newest_adjusted(&editor.display_snapshot(cx))
            .range();
        let buffer = editor.buffer().read(cx).as_singleton()?;
        let snapshot = buffer.read(cx).snapshot();
        let (ranges, next_point) = repl::runnable_ranges(&snapshot, selection, cx);
        let code = ranges
            .into_iter()
            .map(|range| snapshot.text_for_range(range).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");
        let multibuffer_snapshot = editor.buffer().read(cx).read(cx);
        let next_anchor = next_point.map(|point| multibuffer_snapshot.anchor_after(point));
        Some((code, next_anchor))
    });
    let Some((code, next_anchor)) = picked else {
        return;
    };
    if code.trim().is_empty() {
        return;
    }

    terminal_view.update(cx, |terminal_view, cx| {
        terminal_view.terminal().update(cx, |terminal, _| {
            terminal.paste(&code);
            terminal.input(b"\r".to_vec());
        });
    });

    if let Some(next_anchor) = next_anchor {
        editor.update(cx, |editor, cx| {
            editor.change_selections(
                SelectionEffects::scroll(Autoscroll::fit()),
                window,
                cx,
                |selections| selections.select_ranges([next_anchor..next_anchor]),
            );
        });
    }
}
