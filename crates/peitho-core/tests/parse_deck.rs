use peitho_core::domain::FragmentKind;

#[test]
fn parse_deck_is_public_and_leaves_code_images_untransformed() {
    let source = r#"---
code_images:
  dot: dot -Tsvg
---
# Diagram

```dot
digraph {
  A -> B;
}
```

<!-- note -->
"#;
    let frontmatter = peitho_core::parse_frontmatter(source).unwrap();
    let deck = peitho_core::parse_deck(
        source,
        frontmatter,
        &peitho_core::highlight::Highlighter::defaults(),
    )
    .unwrap();
    let slide = &deck.parsed_slides()[0];
    let fragment = &slide.fragments[1];

    assert!(matches!(fragment.kind(), FragmentKind::Code));
    assert!(fragment.code_text().contains("digraph {\n  A -> B;\n}"));
    assert_eq!(slide.note_spans.len(), 1);
}
