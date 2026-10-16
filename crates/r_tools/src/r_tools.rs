mod plot_view;
mod variables_panel;

use std::sync::Arc;

use collections::HashMap;

use editor::{Editor, SelectionEffects, scroll::Autoscroll};
use gpui::{App, AppContext as _, Context, Entity, EventEmitter, Subscription, Window, actions};
use jupyter_protocol::JupyterMessage;
use repl::{Kernel, KernelMessage, ReplStore, Session, SessionEvent};
use task::{
    HideStrategy, RevealStrategy, RevealTarget, SaveStrategy, Shell, SpawnInTerminal, TaskId,
};
use terminal_view::{TerminalView, terminal_panel::TerminalPanel};
use workspace::{Toast, Workspace, dock::Panel as _, notifications::NotificationId};

pub use plot_view::PlotView;
pub use variables_panel::VariablesPanel;

actions!(
    r_tools,
    [
        /// Toggles the R variables panel.
        ToggleVariables,
        /// Opens the R plots pane in a split to the right, or activates it if it is open.
        OpenPlots,
        /// Sends the selection, or the top-level R statement at the cursor, to the active terminal if it runs R.
        SendToTerminal,
        /// Opens a Jupyter console in a terminal, attached to the R session.
        OpenConsole,
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
        let panel = cx.new(|cx| VariablesPanel::new(tracker.clone(), cx));
        workspace.add_panel(panel, window, cx);

        workspace.register_action(|workspace, _: &ToggleVariables, window, cx| {
            workspace.toggle_panel_focus::<VariablesPanel>(window, cx);
        });
        workspace.register_action({
            let tracker = tracker.clone();
            move |workspace, _: &OpenPlots, window, cx| {
                plot_view::open_plot_view(workspace, &tracker, window, cx);
            }
        });
        workspace.register_action(move |workspace, _: &OpenConsole, window, cx| {
            open_console(workspace, &tracker, window, cx);
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

/// Runs `jupyter-console --existing` through uvx on the R kernel's connection
/// file, so the console shares the session with the editor, the plots pane and
/// the variables panel. uvx fetches jupyter-console on first use. jupyter-console
/// can also echo other clients' input (`include_other_output`), but that adds
/// stray prompts, so it stays off; SendToTerminal sends editor code to the
/// console instead.
fn open_console(
    workspace: &mut Workspace,
    tracker: &Entity<SessionTracker>,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    struct NoRSession;

    let kernel_files =
        tracker
            .read(cx)
            .running_session(cx)
            .and_then(|session| match &session.read(cx).kernel {
                Kernel::RunningKernel(kernel) => Some((
                    kernel.connection_file()?.to_path_buf(),
                    kernel.working_directory().clone(),
                )),
                _ => None,
            });
    let Some((connection_file, working_directory)) = kernel_files else {
        workspace.show_toast(
            Toast::new(
                NotificationId::unique::<NoRSession>(),
                "Run some R code to start the R session.",
            )
            .autohide(),
            cx,
        );
        return;
    };

    // The terminal joins command and arguments into one shell command line,
    // so a path with spaces must be quoted for the system shell it runs in.
    let Some(connection_file_arg) = util::shell::ShellKind::system()
        .try_quote(&connection_file.to_string_lossy())
        .map(|quoted| quoted.into_owned())
    else {
        log::error!("cannot quote the kernel connection file {connection_file:?} for the shell");
        return;
    };
    let args = vec![
        "--from".to_string(),
        "jupyter-console".to_string(),
        "jupyter-console".to_string(),
        "--existing".to_string(),
        connection_file_arg,
    ];
    let command_label = format!("uvx {}", args.join(" "));
    let spawn = SpawnInTerminal {
        id: TaskId("r_tools_console".to_string()),
        full_label: command_label.clone(),
        label: "R console".to_string(),
        command: Some("uvx".to_string()),
        args,
        command_label,
        cwd: Some(working_directory),
        env: HashMap::default(),
        use_new_terminal: true,
        allow_concurrent_runs: true,
        reveal: RevealStrategy::Always,
        reveal_target: RevealTarget::Dock,
        hide: HideStrategy::Never,
        shell: Shell::System,
        show_summary: false,
        show_command: false,
        show_rerun: false,
        save: SaveStrategy::default(),
    };
    workspace.spawn_in_terminal(spawn, window, cx).detach();
}

fn send_to_terminal(
    workspace: &mut Workspace,
    _: &SendToTerminal,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    struct NoTerminal;

    /// Whether a terminal's foreground process is an R console: R, radian, or
    /// the jupyter-console that OpenConsole starts. Code is pasted only into
    /// these, so a shell never runs R code (where `x > y` would overwrite `y`).
    fn is_r_console(name: &str, argv: &[String]) -> bool {
        const CONSOLES: [&str; 4] = ["r", "rterm", "radian", "jupyter-console"];
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
                "The active terminal is not running R. Start R, radian or the R console there first.",
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

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Focusable as _, TestAppContext};
    use project::{FakeFs, Project};
    use serde_json::json;
    use settings::SettingsStore;

    const ONE_PIXEL_PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

    fn plot_message(msg_type: &str) -> Arc<JupyterMessage> {
        let content = jupyter_protocol::JupyterMessageContent::from_type_and_content(
            msg_type,
            json!({
                "data": { "image/png": ONE_PIXEL_PNG },
                "metadata": {},
                "transient": { "display_id": "plot-1" }
            }),
        );
        match content {
            Ok(content) => Arc::new(JupyterMessage::new(content, None)),
            Err(error) => panic!("invalid {msg_type} fixture: {error}"),
        }
    }

    async fn r_tools_workspace(
        cx: &mut TestAppContext,
    ) -> (Entity<Workspace>, &mut gpui::VisualTestContext) {
        cx.update(|cx| {
            let settings_store = SettingsStore::test(cx);
            cx.set_global(settings_store);
            theme_settings::init(theme::LoadThemes::JustBase, cx);
            init(cx);
        });
        let project = Project::test(FakeFs::new(cx.executor()), None, cx).await;
        cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx))
    }

    #[gpui::test]
    async fn test_variables_panel_moves_to_chosen_dock(cx: &mut TestAppContext) {
        let (workspace, cx) = r_tools_workspace(cx).await;
        let panel = workspace.read_with(cx, |workspace, cx| {
            match workspace.panel::<VariablesPanel>(cx) {
                Some(panel) => panel,
                None => panic!("variables panel was not registered"),
            }
        });

        panel.update_in(cx, |panel, window, cx| {
            panel.set_position(workspace::dock::DockPosition::Left, window, cx)
        });
        cx.run_until_parked();

        workspace.read_with(cx, |workspace, cx| {
            assert!(
                workspace
                    .left_dock()
                    .read(cx)
                    .panel::<VariablesPanel>()
                    .is_some()
            );
            assert!(
                workspace
                    .right_dock()
                    .read(cx)
                    .panel::<VariablesPanel>()
                    .is_none()
            );
        });
        cx.run_until_parked();
        let saved = cx
            .update(|_, cx| db::kvp::KeyValueStore::global(cx).read_kvp("r_variables_panel_dock"));
        assert_eq!(saved.ok().flatten().as_deref(), Some("left"));

        let tracker = panel.read_with(cx, |panel, _| panel.tracker().clone());
        let reopened = cx.update(|_, cx| cx.new(|cx| VariablesPanel::new(tracker, cx)));
        cx.update(|window, cx| {
            assert_eq!(
                reopened.read(cx).position(window, cx),
                workspace::dock::DockPosition::Left
            );
        });
    }

    #[gpui::test]
    async fn test_first_plot_opens_split_without_stealing_focus(cx: &mut TestAppContext) {
        let (workspace, cx) = r_tools_workspace(cx).await;

        let tracker = workspace.read_with(cx, |workspace, cx| {
            match workspace.panel::<VariablesPanel>(cx) {
                Some(panel) => panel.read(cx).tracker().clone(),
                None => panic!("variables panel was not registered"),
            }
        });
        let original_pane = workspace.update_in(cx, |workspace, window, cx| {
            let pane = workspace.active_pane().clone();
            let focus_handle = pane.focus_handle(cx);
            window.focus(&focus_handle, cx);
            pane
        });

        tracker.update(cx, |_, cx| {
            cx.emit(TrackerEvent::KernelMessage(plot_message("display_data")))
        });
        cx.run_until_parked();
        tracker.update(cx, |_, cx| {
            cx.emit(TrackerEvent::KernelMessage(plot_message(
                "update_display_data",
            )))
        });
        cx.run_until_parked();

        let original_focus = original_pane.read_with(cx, |pane, cx| pane.focus_handle(cx));
        cx.update(|window, cx| {
            assert!(original_focus.contains_focused(window, cx));
        });
        workspace.read_with(cx, |workspace, cx| {
            let plot_views = workspace.items_of_type::<PlotView>(cx).collect::<Vec<_>>();
            assert_eq!(plot_views.len(), 1);
            assert_eq!(plot_views[0].read(cx).plot_count(), 1);
            assert_eq!(workspace.panes().len(), 2);
            assert_eq!(workspace.active_pane(), &original_pane);
        });

        cx.dispatch_action(OpenPlots);
        cx.run_until_parked();
        workspace.read_with(cx, |workspace, cx| {
            assert_eq!(workspace.items_of_type::<PlotView>(cx).count(), 1);
            assert_eq!(workspace.panes().len(), 2);
        });
    }
}
