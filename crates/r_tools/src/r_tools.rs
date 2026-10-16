use editor::{Bias, Editor, SelectionEffects, scroll::Autoscroll};
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

/// Whether a terminal's foreground process is a REPL that takes pasted code:
/// R, radian, Python, IPython or Julia. Code is pasted only into these, so a
/// shell never runs it (where `x > y` would overwrite `y`), and neither does a
/// program that merely names one, such as `vim julia` or `python train.py`.
fn is_repl(name: &str, argv: &[String]) -> bool {
    const REPLS: [&str; 6] = ["r", "rterm", "radian", "ipython", "ipython3", "julia"];
    const PYTHON_REPL_MODULES: [&str; 2] = ["ipython", "radian"];
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
    let mut programs = std::iter::once(name).chain(argv.first().map(String::as_str));
    let args = argv.get(1..).unwrap_or_default();

    if programs.clone().any(|arg| is_one_of(arg, &REPLS)) {
        return true;
    }
    if !programs.any(|arg| program(arg).is_some_and(|p| p == "python" || p.starts_with("python3")))
    {
        return false;
    }
    // `python` alone, `python -m IPython` (radian restarts itself as
    // `python -P -m radian`), or an entry-point script such as
    // radian or ipython; not `python train.py`.
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
    true
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
    let Some(terminal_view) = terminal_view.filter(|_| runs_repl) else {
        workspace.show_toast(
            Toast::new(
                NotificationId::unique::<NoTerminal>(),
                "The active terminal is not running a REPL. Start R, radian, Python, IPython or Julia there first.",
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
