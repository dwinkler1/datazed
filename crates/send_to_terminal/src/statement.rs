use std::ops::Range;

use language::{BufferSnapshot, Point};

/// The top-level statement at `cursor`, read from the buffer's syntax tree:
/// the root node's named children, without comments. Works for any language
/// with a grammar. A `;` right after a statement belongs to it.
///
/// On a blank or comment line, the next statement is chosen; behind a
/// statement on the same line, such as after a trailing comment, that
/// statement. Returns `None` when the buffer has no syntax tree, the cursor is
/// past the last statement, or the statement contains a syntax error.
pub(crate) fn top_level_statement_at(
    buffer: &BufferSnapshot,
    cursor: Point,
) -> Option<Range<Point>> {
    let root = buffer.syntax_layers().next()?.node();
    let mut tree_cursor = root.walk();
    let mut statements = Vec::new();
    if tree_cursor.goto_first_child() {
        loop {
            let node = tree_cursor.node();
            // Grammars list comments as extras, so this skips them in any language.
            if node.is_named() && !node.is_extra() {
                let range = node.start_byte()..with_trailing_semicolon(buffer, node.end_byte());
                statements.push((range, node.has_error()));
            }
            if !tree_cursor.goto_next_sibling() {
                break;
            }
        }
    }

    let row_of = |offset: usize| buffer.offset_to_point(offset).row;
    let cursor_offset = buffer.point_to_offset(cursor);
    let containing_or_after = statements
        .iter()
        .position(|(statement, _)| statement.end >= cursor_offset);
    let last_ending_on_cursor_row = statements
        .iter()
        .rposition(|(statement, _)| row_of(statement.end) == cursor.row);
    let target = match (containing_or_after, last_ending_on_cursor_row) {
        (Some(after), Some(on_row)) if row_of(statements[after].0.start) > cursor.row => on_row,
        (Some(after), _) => after,
        (None, on_row) => on_row?,
    };

    let (statement, has_error) = &statements[target];
    // A syntax error can make one node span the rest of the buffer.
    if *has_error {
        return None;
    }
    Some(buffer.offset_to_point(statement.start)..buffer.offset_to_point(statement.end))
}

/// Grammars leave a statement's `;` outside its node, but some REPLs read it:
/// IPython and Julia do not print the result of code that ends in `;`.
fn with_trailing_semicolon(buffer: &BufferSnapshot, end: usize) -> usize {
    let mut offset = end;
    for c in buffer.chars_at(end) {
        match c {
            ' ' | '\t' => offset += c.len_utf8(),
            ';' => return offset + 1,
            _ => break,
        }
    }
    end
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AppContext as _, TestAppContext};
    use language::{Buffer, Language, LanguageConfig};
    use std::sync::Arc;

    fn statement(
        language: Option<Arc<Language>>,
        source: &str,
        cursor: Point,
        cx: &mut TestAppContext,
    ) -> Option<String> {
        let buffer = cx.new(|cx| {
            let buffer = Buffer::local(source, cx);
            match language {
                Some(language) => buffer.with_language(language, cx),
                None => buffer,
            }
        });
        cx.run_until_parked();
        buffer.read_with(cx, |buffer, _| {
            let snapshot = buffer.snapshot();
            top_level_statement_at(&snapshot, cursor)
                .map(|range| snapshot.text_for_range(range).collect())
        })
    }

    fn python() -> Option<Arc<Language>> {
        let config = LanguageConfig {
            name: "Python".into(),
            ..Default::default()
        };
        let grammar = language::tree_sitter_python::LANGUAGE.into();
        Some(Arc::new(Language::new(config, Some(grammar))))
    }

    fn rust() -> Option<Arc<Language>> {
        let config = LanguageConfig {
            name: "Rust".into(),
            ..Default::default()
        };
        let grammar = language::tree_sitter_rust::LANGUAGE.into();
        Some(Arc::new(Language::new(config, Some(grammar))))
    }

    #[gpui::test]
    fn test_python_statements(cx: &mut TestAppContext) {
        let source = indoc::indoc! {r#"
            import os

            # Squares its input
            def square(x):
                y = x * x
                return y
            # a comment between statements

            result = square(4)  # trailing comment
            a = 1; b = 2
            df = load() ;
        "#};
        let square = "def square(x):\n    y = x * x\n    return y";
        let cases: &[(&str, Point, Option<&str>)] = &[
            ("first statement", Point::new(0, 0), Some("import os")),
            ("blank line", Point::new(1, 0), Some(square)),
            ("comment line", Point::new(2, 3), Some(square)),
            ("inside a function body", Point::new(4, 6), Some(square)),
            (
                "after a trailing comment",
                Point::new(8, 35),
                Some("result = square(4)"),
            ),
            (
                "first of two statements on a row",
                Point::new(9, 0),
                Some("a = 1;"),
            ),
            (
                "second of two statements on a row",
                Point::new(9, 7),
                Some("b = 2"),
            ),
            (
                "trailing semicolon after a space",
                Point::new(10, 0),
                Some("df = load() ;"),
            ),
            ("past the last statement", Point::new(11, 0), None),
        ];
        for (name, cursor, expected) in cases {
            assert_eq!(
                statement(python(), source, *cursor, cx).as_deref(),
                *expected,
                "{name}"
            );
        }
    }

    #[gpui::test]
    fn test_any_grammar(cx: &mut TestAppContext) {
        let source = "use std::fmt;\n\nfn main() {\n    println!(\"hi\");\n}\n";
        assert_eq!(
            statement(rust(), source, Point::new(3, 4), cx).as_deref(),
            Some("fn main() {\n    println!(\"hi\");\n}")
        );
    }

    #[gpui::test]
    fn test_no_statement(cx: &mut TestAppContext) {
        // The unclosed `(` puts an error inside the function definition, which
        // here spans the rest of the buffer.
        let broken = "def f():\n    return (1,\n\ny = 3\n";
        assert_eq!(statement(python(), broken, Point::new(1, 4), cx), None);
        assert_eq!(statement(None, "x = 1\n", Point::new(0, 0), cx), None);
    }
}
