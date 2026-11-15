//! Preserve source coordinates while adapting TypeScript 5 type-only star exports to the bundled
//! grammar.
//!
//! tree-sitter-typescript 0.23.2 parses the `type` keyword in these two forms as an ERROR (upstream
//! #360). Erasing only that exact error token inside an otherwise ordinary star re-export lets the
//! existing grammar validate the entire statement and file. Strings, comments, other errors and all
//! byte offsets stay intact.

use tree_sitter::{Node, Parser, Tree};

pub(super) fn parse(parser: &mut Parser, source: &[u8]) -> Option<Tree> {
    let tree = parser.parse(source, None)?;
    if !tree.root_node().has_error() {
        return Some(tree);
    }
    let mut normalized = source.to_vec();
    let mut changed = false;
    erase_type_only_export_keywords(tree.root_node(), source, &mut normalized, &mut changed);
    if changed {
        parser.parse(&normalized, None)
    } else {
        Some(tree)
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
    fn test_should_keep_real_syntax_errors() {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into())
            .expect("grammar");
        for source in [
            "export type * from;",
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
