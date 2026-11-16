//! Preserve source coordinates while adapting unsupported TypeScript syntax to the bundled grammar.
//!
//! tree-sitter-typescript 0.23.2 parses the `type` keyword in these two forms as an ERROR (upstream
//! #360). Erasing only that exact error token inside an otherwise ordinary star re-export lets the
//! existing grammar validate the entire statement and file. Awaited generic calls with import-type
//! arguments have a separate narrow adapter. Both reparse the entire file and retain byte/line
//! coordinates.

use tree_sitter::{Node, Parser, Tree};

pub(super) fn parse(parser: &mut Parser, source: &[u8]) -> Option<Tree> {
    let tree = parser.parse(source, None)?;
    if !tree.root_node().has_error() {
        return Some(tree);
    }
    let mut normalized = source.to_vec();
    let mut changed = false;
    erase_import_query_arguments(tree.root_node(), &mut normalized, &mut changed);
    erase_type_only_export_keywords(tree.root_node(), source, &mut normalized, &mut changed);
    if changed {
        parser.parse(&normalized, None)
    } else {
        Some(tree)
    }
}

// The grammar interprets `await load<typeof import("./types")>()` as a comparison and leaves `>()`
// as ERROR. Replace only a single-string import type in that comparison position with a same-width
// type identifier; parsing the entire result still rejects malformed calls and unrelated syntax
// errors.
fn erase_import_query_arguments(node: Node<'_>, normalized: &mut [u8], changed: &mut bool) {
    if node.kind() == "unary_expression"
        && node.child(0).is_some_and(|n| n.kind() == "typeof")
        && let Some(call) = node.child_by_field_name("argument")
        && call.kind() == "call_expression"
        && call
            .child_by_field_name("function")
            .is_some_and(|n| n.kind() == "import")
        && let Some(args) = call.child_by_field_name("arguments")
        && args.named_child_count() == 1
        && args.named_child(0).is_some_and(|n| n.kind() == "string")
        && node.parent().is_some_and(|p| {
            p.kind() == "binary_expression"
                && p.child_by_field_name("operator")
                    .is_some_and(|n| n.kind() == "<")
        })
        && let Some(bytes) = normalized.get_mut(node.byte_range())
    {
        for byte in bytes.iter_mut() {
            if !matches!(*byte, b'\n' | b'\r') {
                *byte = b' ';
            }
        }
        if let Some(first) = bytes.first_mut() {
            *first = b'T';
        }
        *changed = true;
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        erase_import_query_arguments(child, normalized, changed);
    }
}

fn erase_type_only_export_keywords(
    node: Node<'_>,
    source: &[u8],
    normalized: &mut [u8],
    changed: &mut bool,
) {
    if !node.has_error() {
        return;
    }
    if node.kind() == "export_statement" {
        let mut cursor = node.walk();
        let children: Vec<_> = node
            .children(&mut cursor)
            .filter(|child| child.kind() != "comment")
            .collect();
        if let [export, keyword, star, ..] = children.as_slice()
            && export.kind() == "export"
            && keyword.is_error()
            && source.get(keyword.byte_range()) == Some(b"type".as_slice())
            && matches!(star.kind(), "*" | "namespace_export")
            && let Some(bytes) = normalized.get_mut(keyword.byte_range())
        {
            bytes.fill(b' ');
            *changed = true;
        }
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        erase_type_only_export_keywords(child, source, normalized, changed);
    }
}

#[cfg(test)]
mod tests {
    use tree_sitter::Parser;

    use super::parse;

    #[test]
    fn test_should_parse_type_only_star_exports_with_both_grammars() {
        for language in [
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT,
            tree_sitter_typescript::LANGUAGE_TSX,
        ] {
            let mut parser = Parser::new();
            parser.set_language(&language.into()).expect("grammar");
            for source in [
                "export type * from './types';",
                "export type * as Types from './types';",
                "export /* before */ type /* after */ * from './types';",
                "export type /* 中文 */ * as Types from './types';",
                "export type * from './a';\nexport type * as B from './b';",
            ] {
                let tree = parse(&mut parser, source.as_bytes()).expect("syntax tree");
                assert!(
                    !tree.root_node().has_error(),
                    "{source}: {}",
                    tree.root_node().to_sexp()
                );
                assert_eq!(tree.root_node().end_byte(), source.len());
            }
        }
    }

    #[test]
    fn test_should_parse_import_type_arguments_after_await_without_losing_coordinates() {
        for language in [
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT,
            tree_sitter_typescript::LANGUAGE_TSX,
        ] {
            let mut parser = Parser::new();
            parser.set_language(&language.into()).expect("grammar");
            let source = concat!(
                "const value = (await load<typeof\n",
                "import('./types')>());\n",
                "// 中文\n",
                "function after() { return value; }"
            );
            let tree = parse(&mut parser, source.as_bytes()).expect("tree");
            assert!(
                !tree.root_node().has_error(),
                "{}",
                tree.root_node().to_sexp()
            );
            assert_eq!(tree.root_node().end_byte(), source.len());
            let mut cursor = tree.root_node().walk();
            let function = tree
                .root_node()
                .named_children(&mut cursor)
                .find(|n| n.kind() == "function_declaration")
                .expect("function");
            assert_eq!(
                function.start_byte(),
                source.find("function after").expect("offset")
            );
            assert_eq!(function.start_position().row, 3);
        }
    }

    #[test]
    fn test_should_keep_real_syntax_errors() {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into())
            .expect("grammar");
        for source in [
            "export type * from;",
            "const value = (await load<typeof import('./types')>());\nfunction broken( {",
            "const value = (await load<typeof import()>());",
            "export type * as from './types';",
            "export type * as Types './types';",
            "export typo * from './types';",
            "export default type * from './types';",
            "export type * from './types';\nfunction broken( {",
            "const text = 'export type * from';\nfunction broken( {",
            "// export type * from './types';\nfunction broken( {",
        ] {
            let tree = parse(&mut parser, source.as_bytes()).expect("syntax tree");
            assert!(tree.root_node().has_error(), "{source}");
        }
    }
}
