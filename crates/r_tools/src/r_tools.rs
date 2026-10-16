mod plot_view;
mod variables_panel;

use std::sync::Arc;

use collections::HashMap;

use editor::{Bias, Editor, SelectionEffects, scroll::Autoscroll};
use gpui::{
    App, AppContext as _, Context, Entity, EntityId, EventEmitter, Subscription, Window, actions,
};
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
        /// Toggles the variables panel.
        ToggleVariables,
        /// Opens the plots pane in a split to the right, or activates it if it is open.
        OpenPlots,
        /// Sends the selection, or the top-level statement at the cursor, to the active terminal if it runs a REPL.
        SendToTerminal,
        /// Opens a Jupyter console in a terminal, attached to the REPL session.
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
    /// Sessions with an open console keep sending plots and variable updates
    /// while another editor's session is active.
    consoles: HashMap<EntityId, FollowedConsole>,
    _workspace_subscription: Subscription,
    _watch_editor_for_new_session: Option<Subscription>,
    _session_subscriptions: Vec<Subscription>,
}

struct FollowedConsole {
    session: Entity<Session>,
    open: usize,
    _subscriptions: [Subscription; 2],
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
            consoles: HashMap::default(),
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
        if self.session.as_ref() == Some(&session) {
            return;
        }

        self._session_subscriptions = vec![
            cx.subscribe(&session, |_this, _session, event: &KernelMessage, cx| {
                cx.emit(TrackerEvent::KernelMessage(event.0.clone()));
            }),
            cx.subscribe(&session, |this, session, event: &SessionEvent, cx| {
                let SessionEvent::Shutdown(_) = event;
                session.update(cx, |session, _| session.followed = false);
                this.session = None;
                this._session_subscriptions.clear();
                cx.emit(TrackerEvent::SessionChanged);
            }),
            cx.observe(&session, |_this, _session, cx| {
                cx.emit(TrackerEvent::SessionUpdated);
            }),
        ];
        // A followed session emits KernelMessage events and leaves its plots to
        // the plots pane; a session no longer followed shows them inline again.
        if let Some(previous) = self.session.take()
            && !self.consoles.contains_key(&previous.entity_id())
        {
            previous.update(cx, |previous, _| previous.followed = false);
        }
        session.update(cx, |session, _| session.followed = true);
        self.session = Some(session);
        cx.emit(TrackerEvent::SessionChanged);
    }

    fn follow_console(&mut self, session: Entity<Session>, cx: &mut Context<Self>) {
        if let Some(console) = self.consoles.get_mut(&session.entity_id()) {
            console.open += 1;
            return;
        }
        let subscriptions = [
            cx.subscribe(&session, |this, session, event: &KernelMessage, cx| {
                // The active session's messages already arrive through its own
                // subscription.
                if this.session.as_ref() != Some(&session) {
                    cx.emit(TrackerEvent::KernelMessage(event.0.clone()));
                }
            }),
            cx.subscribe(&session, |this, session, event: &SessionEvent, _cx| {
                let SessionEvent::Shutdown(_) = event;
                this.consoles.remove(&session.entity_id());
            }),
        ];
        session.update(cx, |session, _| session.followed = true);
        self.consoles.insert(
            session.entity_id(),
            FollowedConsole {
                session,
                open: 1,
                _subscriptions: subscriptions,
            },
        );
    }

    fn console_closed(&mut self, session: &Entity<Session>, cx: &mut Context<Self>) {
        let id = session.entity_id();
        let Some(console) = self.consoles.get_mut(&id) else {
            return;
        };
        console.open -= 1;
        if console.open > 0 {
            return;
        }
        if let Some(console) = self.consoles.remove(&id)
            && self.session.as_ref() != Some(&console.session)
        {
            console
                .session
                .update(cx, |session, _| session.followed = false);
        }
    }
}

/// Runs `jupyter-console --existing` through uvx on the kernel's connection
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
    struct NoSession;

    let kernel_files =
        tracker
            .read(cx)
            .running_session(cx)
            .and_then(|session| match &session.read(cx).kernel {
                Kernel::RunningKernel(kernel) => Some((
                    session.clone(),
                    kernel.connection_file()?.to_path_buf(),
                    kernel.working_directory().clone(),
                )),
                _ => None,
            });
    let Some((session, connection_file, working_directory)) = kernel_files else {
        workspace.show_toast(
            Toast::new(
                NotificationId::unique::<NoSession>(),
                "Run some code to start a kernel session.",
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
        label: "REPL console".to_string(),
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
    let console_exited = workspace.spawn_in_terminal(spawn, window, cx);
    tracker.update(cx, |tracker, cx| {
        tracker.follow_console(session.clone(), cx)
    });
    let tracker = tracker.downgrade();
    let session = session.downgrade();
    cx.spawn(async move |_, cx| {
        console_exited.await;
        if let Some(session) = session.upgrade() {
            tracker
                .update(cx, |tracker, cx| tracker.console_closed(&session, cx))
                .ok();
        }
    })
    .detach();
}

/// Whether a terminal's foreground process is a REPL that takes pasted code:
/// R, radian, Python, IPython, Julia, or the jupyter-console that
/// OpenConsole starts. Code is pasted only into these, so a shell never
/// runs it (where `x > y` would overwrite `y`), and neither does a program
/// that merely names one, such as `vim julia` or `python train.py`.
fn is_repl(name: &str, argv: &[String]) -> bool {
    const REPLS: [&str; 7] = [
        "r",
        "rterm",
        "radian",
        "ipython",
        "ipython3",
        "julia",
        "jupyter-console",
    ];
    const PYTHON_REPL_MODULES: [&str; 3] = ["ipython", "jupyter_console", "radian"];
    fn program(arg: &str) -> Option<String> {
        let file_name = std::path::Path::new(arg).file_name()?.to_str()?;
        Some(
            file_name
                .to_ascii_lowercase()
                .trim_end_matches(".exe")
                .to_string(),
        )
    }
    let is_one_of =
        |arg: &str, names: &[&str]| program(arg).is_some_and(|p| names.contains(&p.as_str()));
    let programs = std::iter::once(name).chain(argv.first().map(String::as_str));
    let args = argv.get(1..).unwrap_or_default();

    if programs.clone().any(|arg| is_one_of(arg, &REPLS)) {
        return true;
    }
    if programs
        .clone()
        .any(|arg| program(arg).is_some_and(|p| p == "python" || p.starts_with("python3")))
    {
        // `python` alone, `python -m IPython` (radian restarts itself as
        // `python -P -m radian`), or an entry-point script such
        // as radian or ipython; not `python train.py`.
        let mut args = args.iter();
        while let Some(arg) = args.next() {
            if arg == "-m" {
                return args.next().is_some_and(|module| {
                    PYTHON_REPL_MODULES.contains(&module.to_ascii_lowercase().as_str())
                });
            }
            if !arg.starts_with('-') {
                return is_one_of(arg, &REPLS);
            }
        }
        return true;
    }
    // OpenConsole starts `uvx --from jupyter-console jupyter-console`, and uvx
    // stays the terminal's foreground process while the console runs.
    programs
        .filter_map(program)
        .find_map(|launcher| tool_run_by_uv(&launcher, args))
        .is_some_and(|tool| is_one_of(tool, &REPLS))
}

/// The tool that `uvx <tool>`, `uv run <tool>` or `uv tool run <tool>` runs;
/// uvx itself becomes `uv tool uvx <tool>`.
fn tool_run_by_uv<'a>(launcher: &str, args: &'a [String]) -> Option<&'a str> {
    const OPTIONS_WITH_VALUES: [&str; 5] =
        ["--from", "--with", "--python", "-p", "--with-requirements"];
    let mut positional = Vec::new();
    let mut args = args.iter().map(String::as_str);
    while let Some(arg) = args.next() {
        if OPTIONS_WITH_VALUES.contains(&arg) {
            args.next();
        } else if !arg.starts_with('-') {
            positional.push(arg);
        }
    }
    match (launcher, positional.as_slice()) {
        ("uvx", [tool, ..])
        | ("uv", ["run", tool, ..])
        | ("uv", ["tool", "run" | "uvx", tool, ..]) => Some(tool),
        _ => None,
    }
}

fn send_to_terminal(
    workspace: &mut Workspace,
    _: &SendToTerminal,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    struct NoTerminal;

    let Some(editor) = workspace.active_item_as::<Editor>(cx) else {
        return;
    };
    let terminal_view = workspace
        .panel::<TerminalPanel>(cx)
        .and_then(|panel| panel.read(cx).pane())
        .and_then(|pane| pane.read(cx).active_item())
        .and_then(|item| item.downcast::<TerminalView>());
    let runs_repl = terminal_view.as_ref().is_some_and(|terminal_view| {
        terminal_view
            .read(cx)
            .terminal()
            .read(cx)
            .foreground_process()
            .is_some_and(|(name, argv)| is_repl(&name, &argv))
    });
    let message = match &terminal_view {
        None => Some(
            "No terminal in the terminal panel. Open one and start R, radian, Python, IPython or Julia there, or open the REPL console.",
        ),
        Some(_) if !runs_repl => Some(
            "The active terminal is not running a REPL. Start R, radian, Python, IPython or Julia there, or open the REPL console.",
        ),
        Some(_) => None,
    };
    if let Some(message) = message {
        workspace.show_toast(
            Toast::new(NotificationId::unique::<NoTerminal>(), message).autohide(),
            cx,
        );
        return;
    }
    let Some(terminal_view) = terminal_view else {
        return;
    };

    let picked = editor.update(cx, |editor, cx| {
        let selection = editor
            .selections
            .newest_adjusted(&editor.display_snapshot(cx))
            .range();
        let buffer = editor.buffer().read(cx).as_singleton()?;
        let snapshot = buffer.read(cx).snapshot();
        let (ranges, next_point) = repl::runnable_ranges(&snapshot, selection.clone(), cx);
        // Without a next statement, step past what was sent, as Run does; a
        // selection keeps the cursor where it is.
        let next_point = next_point.or_else(|| {
            let mut after = ranges.last()?.end;
            after.row += 1;
            after.column = 0;
            (selection.start == selection.end).then(|| snapshot.clip_point(after, Bias::Left))
        });
        let code = ranges
            .into_iter()
            .map(|range| snapshot.text_for_range(range).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");
        let is_python = snapshot
            .language()
            .is_some_and(|language| language.name().as_ref() == "Python");
        let multibuffer_snapshot = editor.buffer().read(cx).read(cx);
        let next_anchor = next_point.map(|point| multibuffer_snapshot.anchor_after(point));
        Some((code, next_anchor, is_python))
    });
    let Some((mut code, next_anchor, is_python)) = picked else {
        return;
    };
    if code.trim().is_empty() {
        return;
    }

    // Python and IPython run a compound statement, even one on a single line
    // such as `if x: f()`, only after an empty line.
    if code.contains('\n') || is_python {
        code.push('\n');
    }

    let terminal = terminal_view.read(cx).terminal().clone();
    terminal.update(cx, |terminal, _| terminal.paste(&code));
    // Python's REPL from 3.14 drops an Enter that arrives together with the end of
    // a paste, so every send would wait at `...`; 5 ms apart is enough.
    let terminal = terminal.downgrade();
    cx.spawn(async move |_, cx| {
        cx.background_executor()
            .timer(std::time::Duration::from_millis(50))
            .await;
        terminal
            .update(cx, |terminal, _| terminal.input(b"\r".to_vec()))
            .ok();
    })
    .detach();
    // Show a closed terminal panel without taking focus from the editor.
    workspace.open_panel::<TerminalPanel>(window, cx);

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

    #[test]
    fn test_is_repl() {
        let process = |command: &str| {
            let argv: Vec<String> = command.split(' ').map(String::from).collect();
            let name = std::path::Path::new(&argv[0])
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .to_string();
            is_repl(&name, &argv)
        };
        let repls = [
            "R",
            "/Library/Frameworks/R.framework/Resources/bin/exec/R --no-save",
            "Rterm.exe",
            "julia --project",
            "python3",
            "python3.12 -q",
            "python3.12 /Users/me/.local/bin/radian",
            "/opt/radian/bin/python -P -m radian --quiet",
            "python3 -m IPython",
            "uvx --from jupyter-console jupyter-console --existing /tmp/kernel.json",
            "/opt/uv/uv tool uvx --from jupyter-console jupyter-console --existing /tmp/kernel.json",
        ];
        let not_repls = [
            "zsh",
            "vim julia",
            "less python",
            "git log -- r",
            "python train.py",
            "python3 -c print(1)",
            "uv pip install ipython",
            "uvx ruff check",
        ];
        for command in repls {
            assert!(process(command), "{command} should take pasted code");
        }
        for command in not_repls {
            assert!(!process(command), "{command} should not take pasted code");
        }
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
