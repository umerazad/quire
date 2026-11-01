//! This crate provides Markdown language support for the [tree-sitter][] parsing library.
//!
//! It contains two grammars: [`LANGUAGE`] to parse the block structure of markdown documents and
//! [`INLINE_LANGUAGE`] to parse inline content.
//!
//! [LanguageFn]: https://docs.rs/tree-sitter-language/*/tree_sitter_language/struct.LanguageFn.html
//! [Tree]: https://docs.rs/tree-sitter/*/tree_sitter/struct.Tree.html
//! [tree-sitter]: https://tree-sitter.github.io/

#![cfg_attr(docsrs, feature(doc_cfg))]

use tree_sitter_language::LanguageFn;

extern "C" {
    fn tree_sitter_quire_markdown() -> *const ();
    fn tree_sitter_quire_markdown_inline() -> *const ();
}

/// The tree-sitter [`LanguageFn`][LanguageFn] for the block grammar.
pub const LANGUAGE: LanguageFn = unsafe { LanguageFn::from_raw(tree_sitter_quire_markdown) };

/// The tree-sitter [`LanguageFn`][LanguageFn] for the inline grammar.
pub const INLINE_LANGUAGE: LanguageFn =
    unsafe { LanguageFn::from_raw(tree_sitter_quire_markdown_inline) };

/// The syntax highlighting queries for the block grammar.
pub const HIGHLIGHT_QUERY_BLOCK: &str =
    include_str!("../../tree-sitter-markdown/queries/highlights.scm");

/// The language injection queries for the block grammar.
pub const INJECTION_QUERY_BLOCK: &str =
    include_str!("../../tree-sitter-markdown/queries/injections.scm");

/// The syntax highlighting queries for the inline grammar.
pub const HIGHLIGHT_QUERY_INLINE: &str =
    include_str!("../../tree-sitter-markdown-inline/queries/highlights.scm");

/// The language injection queries for the inline grammar.
pub const INJECTION_QUERY_INLINE: &str =
    include_str!("../../tree-sitter-markdown-inline/queries/injections.scm");

/// The content of the [`node-types.json`][] file for the block grammar.
///
/// [`node-types.json`]: https://tree-sitter.github.io/tree-sitter/using-parsers#static-node-types
pub const NODE_TYPES_BLOCK: &str = include_str!("../../tree-sitter-markdown/src/node-types.json");

/// The content of the [`node-types.json`][] file for the inline grammar.
///
/// [`node-types.json`]: https://tree-sitter.github.io/tree-sitter/using-parsers#static-node-types
pub const NODE_TYPES_INLINE: &str =
    include_str!("../../tree-sitter-markdown-inline/src/node-types.json");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn can_load_block_grammar() {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&LANGUAGE.into())
            .expect("Error loading Markdown block grammar");
    }

    #[test]
    fn can_load_inline_grammar() {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&INLINE_LANGUAGE.into())
            .expect("Error loading Markdown inline grammar");
    }

    fn parse_nested_block_quotes(depth: usize) -> tree_sitter::Tree {
        let mut text = String::new();
        for line in 0..depth {
            text.push_str(&"> ".repeat(line + 1));
            text.push_str("a\n");
        }
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&LANGUAGE.into())
            .expect("Error loading Markdown block grammar");
        parser
            .parse(&text, None)
            .expect("parsing nested block quotes should produce a tree")
    }

    #[test]
    fn deeply_nested_blocks_do_not_overflow_scanner_serialization() {
        // 300 nested block quotes keep more open blocks than fit in
        // tree-sitter's fixed serialization buffer. Unguarded, serialize()
        // overflowed it and corrupted the parser (see the guard in scanner.c
        // and upstream issue #243). Guarded, the scanner serializes no state
        // and the pathological input surfaces as ERROR nodes, so assert that
        // the parse completes and the tree is walkable, not that it's clean.
        let tree = parse_nested_block_quotes(300);
        let mut cursor = tree.walk();
        let mut nodes = 0usize;
        'walk: loop {
            nodes += 1;
            if cursor.goto_first_child() {
                continue;
            }
            while !cursor.goto_next_sibling() {
                if !cursor.goto_parent() {
                    break 'walk;
                }
            }
        }
        assert!(nodes > 0);
    }

    #[test]
    fn moderately_nested_blocks_parse_cleanly() {
        // Nesting that fits within the serialization buffer must not be
        // affected by the overflow guard.
        let tree = parse_nested_block_quotes(100);
        assert_eq!(tree.root_node().kind(), "document");
        assert!(!tree.root_node().has_error());
    }

    fn block_parser() -> tree_sitter::Parser {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&LANGUAGE.into())
            .expect("Error loading Markdown block grammar");
        parser
    }

    fn block_tree(text: &str) -> String {
        let tree = block_parser()
            .parse(text, None)
            .expect("parsing should produce a tree");
        let sexp = tree.root_node().to_sexp();
        sexp.strip_prefix("(document (section ")
            .and_then(|rest| rest.strip_suffix("))"))
            .unwrap_or(&sexp)
            .to_owned()
    }

    const ITEM: &str = "(list_marker_minus)";
    const DOT: &str = "(list_marker_dot)";

    fn items(marker: &str, contents: &[&str]) -> String {
        let items: Vec<String> = contents
            .iter()
            .map(|content| format!("(list_item {marker}{content})"))
            .collect();
        format!("(list {})", items.join(" "))
    }

    // Lezer's `isBulletList`/`isOrderedList` with `breaking`: inside any
    // enclosing list of the same kind, an empty item (or, for ordered lists,
    // any number) may interrupt a paragraph; outside, CommonMark rules apply.
    #[test]
    fn empty_items_interrupt_paragraphs_only_inside_a_list_of_their_kind() {
        let paragraph = " (paragraph (inline))";
        let continued = " (paragraph (inline) (block_continuation))";
        let lazy = " (paragraph (inline (block_continuation)))";
        let cases = [
            (
                "- holder ^spot\n  - \n",
                items(ITEM, &[&format!("{continued} {}", items(ITEM, &[""]))]),
            ),
            (
                "- foo\n-\n- bar\n",
                items(ITEM, &[paragraph, "", paragraph]),
            ),
            (
                "> foo\n-\n",
                "(block_quote (block_quote_marker) (paragraph (inline)))".to_owned(),
            ),
            ("1. a\n   - \n", items(DOT, &[lazy])),
            ("- a\n  7. b\n", items(ITEM, &[lazy])),
            ("1. a\n2.\n", items(DOT, &[paragraph, ""])),
            ("a\n2.\n", "(paragraph (inline))".to_owned()),
            (
                "1. a\n   7. b\n",
                items(DOT, &[&format!("{continued} {}", items(DOT, &[paragraph]))]),
            ),
        ];
        for (text, expected) in cases {
            assert_eq!(block_tree(text), expected, "{text:?}");
        }
    }

    #[test]
    fn markers_and_breaks_may_end_at_eof() {
        let cases = [
            (
                "- Parent\n- Sibling\n-",
                items(
                    ITEM,
                    &[" (paragraph (inline))", " (paragraph (inline))", ""],
                ),
            ),
            ("1.", items(DOT, &[""])),
            ("* ", "(list (list_item (list_marker_star)))".to_owned()),
            (
                "> -",
                format!("(block_quote (block_quote_marker) {})", items(ITEM, &[""])),
            ),
            ("a\n***", "(paragraph (inline)) (thematic_break)".to_owned()),
            ("a\n___", "(paragraph (inline)) (thematic_break)".to_owned()),
            ("---", "(thematic_break)".to_owned()),
        ];
        for (text, expected) in cases {
            assert_eq!(block_tree(text), expected, "{text:?}");
        }
        assert_eq!(
            block_parser()
                .parse("", None)
                .unwrap()
                .root_node()
                .to_sexp(),
            "(document)"
        );
    }

    // The list kind of every open item survives scanner serialization, so an
    // incremental reparse agrees with a fresh one.
    #[test]
    fn list_kinds_survive_incremental_reparses() {
        let before = "1. a\n   - b\n   - \n\n- c\n  1. d\n";
        let after = "1. a\n   - b\n   - \n\n- c\n  1. d\n  2.\n";
        let mut parser = block_parser();
        let mut tree = parser.parse(before, None).unwrap();
        tree.edit(&tree_sitter::InputEdit {
            start_byte: before.len(),
            old_end_byte: before.len(),
            new_end_byte: after.len(),
            start_position: tree_sitter::Point::new(6, 0),
            old_end_position: tree_sitter::Point::new(6, 0),
            new_end_position: tree_sitter::Point::new(7, 0),
        });
        let edited = parser.parse(after, Some(&tree)).unwrap();
        assert_eq!(
            edited.root_node().to_sexp(),
            block_parser()
                .parse(after, None)
                .unwrap()
                .root_node()
                .to_sexp()
        );
        assert!(!edited.root_node().has_error());
    }

    const FENCE: &str = "(fenced_code_block (fenced_code_block_delimiter)";

    // Lezer opens a fence on a line ending at EOF and closes one only where a
    // line's content starts: backticks after text on a line are code.
    #[test]
    fn fences_open_at_eof_and_close_only_at_a_line_start() {
        let item = items(ITEM, &[" (paragraph (inline))"]);
        let content = "(block_continuation) (code_fence_content (block_continuation)";
        let cases = [
            ("```", format!("{FENCE})")),
            ("- a\n```", format!("{item} {FENCE})")),
            ("a\n~~~ js", format!("(paragraph (inline)) {FENCE} (info_string (language)))")),
            (
                "```\nx\n```",
                format!("{FENCE} {content}) (fenced_code_block_delimiter))"),
            ),
            (
                "```\nx ```\ny\n```\n",
                format!("{FENCE} {content} (block_continuation)) (fenced_code_block_delimiter))"),
            ),
            (
                "> ~~~\n> x ~~~\n> ~~~\n",
                format!("(block_quote (block_quote_marker) {FENCE} {content}) (fenced_code_block_delimiter)))"),
            ),
            (
                "- a\n  ```\n  x ```\n  ```\n- b\n",
                format!(
                    "(list (list_item (list_marker_minus) (paragraph (inline) (block_continuation)) {FENCE} {content}) (fenced_code_block_delimiter))) (list_item (list_marker_minus) (paragraph (inline))))"
                ),
            ),
        ];
        for (text, expected) in cases {
            assert_eq!(block_tree(text), expected, "{text:?}");
        }
        let text = "```\nx ```\ny\n```\n";
        let mut parser = block_parser();
        let mut tree = parser.parse(text, None).unwrap();
        let at = text.find("y").unwrap();
        tree.edit(&tree_sitter::InputEdit {
            start_byte: at,
            old_end_byte: at,
            new_end_byte: at + 1,
            start_position: tree_sitter::Point::new(2, 0),
            old_end_position: tree_sitter::Point::new(2, 0),
            new_end_position: tree_sitter::Point::new(2, 1),
        });
        let edited = format!("{}z{}", &text[..at], &text[at..]);
        let reparsed = parser.parse(&edited, Some(&tree)).unwrap();
        assert_eq!(
            reparsed.root_node().to_sexp(),
            parser.parse(&edited, None).unwrap().root_node().to_sexp(),
            "an incremental reparse agrees with a fresh one"
        );
    }
}
