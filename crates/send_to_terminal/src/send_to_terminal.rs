use std::ops::Range;

use editor::{Editor, SelectionEffects, scroll::Autoscroll};
use gpui::{App, Context, Entity, Window, actions};
use language::{BufferSnapshot, Point};
use terminal_view::{TerminalView, terminal_panel::TerminalPanel};
use workspace::{Toast, dock::Panel as _, notifications::NotificationId};

actions!(
    send_to_terminal,
    [
        /// Sends the selection, or the line at the cursor, to the active
        /// terminal and moves the cursor to the line after the code it sent.
        SendToTerminal,
    ]
);

pub fn init(cx: &mut App) {
    cx.observe_new(|editor: &mut Editor, _, cx: &mut Context<Editor>| {
        if !editor.mode().is_full() {
            return;
        }
        let handle = cx.entity().downgrade();
        editor
            .register_action(move |_: &SendToTerminal, window, cx| {
                if let Some(editor) = handle.upgrade() {
                    send_to_terminal(&editor, window, cx);
                }
            })
            .detach();
    })
    .detach();
}

fn send_to_terminal(editor: &Entity<Editor>, window: &mut Window, cx: &mut App) {
    struct NoTerminal;

    let Some(workspace) = editor.read(cx).workspace() else {
        return;
    };
    let terminal_view = workspace
        .read(cx)
        .panel::<TerminalPanel>(cx)
        .and_then(|panel| panel.read(cx).pane())
        .and_then(|pane| pane.read(cx).active_item())
        .and_then(|item| item.downcast::<TerminalView>());
    let Some(terminal_view) = terminal_view else {
        workspace.update(cx, |workspace, cx| {
            workspace.show_toast(
                Toast::new(
                    NotificationId::unique::<NoTerminal>(),
                    "No terminal in the terminal panel. Open one there and start the program to send code to.",
                )
                .autohide(),
                cx,
            );
        });
        return;
    };

    let picked = editor.update(cx, |editor, cx| {
        let selection = editor
            .selections
            .newest_adjusted(&editor.display_snapshot(cx))
            .range();
        let buffer = editor.buffer().read(cx).as_singleton()?;
        let snapshot = buffer.read(cx).snapshot();
        let (range, next_point) = code_range(&snapshot, selection);
        let code = snapshot.text_for_range(range).collect::<String>();
        let next_anchor = editor.buffer().read(cx).read(cx).anchor_after(next_point);
        Some((code, next_anchor))
    });
    let Some((mut code, next_anchor)) = picked else {
        return;
    };
    if code.trim().is_empty() {
        return;
    }
    // Python and IPython run a multi-line block only after an empty line.
    if code.contains('\n') {
        code.push('\n');
    }

    terminal_view.update(cx, |terminal_view, cx| {
        terminal_view.terminal().update(cx, |terminal, _| {
            terminal.paste(&code);
            terminal.input(b"\r".to_vec());
        });
    });

    editor.update(cx, |editor, cx| {
        editor.change_selections(
            SelectionEffects::scroll(Autoscroll::fit()),
            window,
            cx,
            |selections| selections.select_ranges([next_anchor..next_anchor]),
        );
    });
}

/// The code to send and where the cursor goes afterwards. The code is the
/// selection, or without one the cursor's line (the next non-blank line when
/// it is blank). The cursor then moves to the start of the following line, so
/// repeated sends step through a file.
fn code_range(buffer: &BufferSnapshot, selection: Range<Point>) -> (Range<Point>, Point) {
    let max_row = buffer.max_point().row;
    let range = if selection.start != selection.end {
        // A line-wise selection ends at column 0 of the following row; that
        // row is not part of the code.
        let mut range = selection;
        if range.end.column == 0 && range.end.row > range.start.row {
            let last_row = range.end.row - 1;
            range.end = Point::new(last_row, buffer.line_len(last_row));
        }
        range
    } else {
        let mut row = selection.start.row;
        while row < max_row && buffer.is_line_blank(row) {
            row += 1;
        }
        Point::new(row, 0)..Point::new(row, buffer.line_len(row))
    };
    let next = if range.end.row < max_row {
        Point::new(range.end.row + 1, 0)
    } else {
        range.end
    };
    (range, next)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AppContext as _, TestAppContext};
    use language::Buffer;

    fn pick(source: &str, selection: Range<Point>, cx: &mut TestAppContext) -> (String, Point) {
        let buffer = cx.new(|cx| Buffer::local(source, cx));
        buffer.read_with(cx, |buffer, _| {
            let snapshot = buffer.snapshot();
            let (range, next) = code_range(&snapshot, selection);
            (snapshot.text_for_range(range).collect(), next)
        })
    }

    fn cursor(row: u32, column: u32) -> Range<Point> {
        Point::new(row, column)..Point::new(row, column)
    }

    #[gpui::test]
    fn test_lines_and_selections(cx: &mut TestAppContext) {
        let source = "x = 1\n\n\ny = 2\nz = 3";
        let cases: &[(&str, Range<Point>, &str, Point)] = &[
            ("cursor line", cursor(0, 3), "x = 1", Point::new(1, 0)),
            (
                "blank line sends the next line",
                cursor(1, 0),
                "y = 2",
                Point::new(4, 0),
            ),
            (
                "selection ending on the last line",
                Point::new(3, 4)..Point::new(4, 1),
                "2\nz",
                Point::new(4, 1),
            ),
            (
                "selection ending mid-buffer",
                Point::new(0, 0)..Point::new(0, 5),
                "x = 1",
                Point::new(1, 0),
            ),
            (
                "line-wise selection drops the row it ends on",
                Point::new(3, 0)..Point::new(4, 0),
                "y = 2",
                Point::new(4, 0),
            ),
            (
                "last line keeps the cursor at its end",
                cursor(4, 0),
                "z = 3",
                Point::new(4, 5),
            ),
        ];
        for (name, selection, expected_code, expected_next) in cases {
            assert_eq!(
                pick(source, selection.clone(), cx),
                (expected_code.to_string(), *expected_next),
                "{name}"
            );
        }
    }
}
