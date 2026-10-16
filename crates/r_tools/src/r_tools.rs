use editor::{Editor, SelectionEffects, scroll::Autoscroll};
use gpui::{App, Context, Window, actions};
use terminal_view::{TerminalView, terminal_panel::TerminalPanel};
use workspace::{Toast, Workspace, dock::Panel as _, notifications::NotificationId};

actions!(
    r_tools,
    [
        /// Sends the current top-level expression or selection to the active terminal.
        SendToTerminal,
    ]
);

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _window, _cx| {
        workspace.register_action(send_to_terminal);
    })
    .detach();
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
