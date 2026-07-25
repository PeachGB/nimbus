use super::*;

#[test]
fn wrap_indented_keeps_short_text_on_one_line() {
    assert_eq!(wrap_indented("hello there", 0, 40), vec!["hello there"]);
}

#[test]
fn wrap_indented_breaks_at_word_boundaries() {
    let lines = wrap_indented("one two three four", 0, 9);
    assert_eq!(lines, vec!["one two", "three", "four"]);
}

#[test]
fn wrap_indented_prefixes_every_line_with_the_indent() {
    let lines = wrap_indented("one two three", 2, 9);
    assert!(lines.iter().all(|line| line.starts_with("  ")));
    assert!(lines.len() > 1);
}

#[test]
fn wrap_indented_breaks_a_word_too_long_to_fit() {
    // An unbroken path in an error message must not overflow the line it's on.
    let lines = wrap_indented("Object not found: aaaaaaaaaaaaaaaaaaaaaaaa", 0, 10);
    assert!(
        lines.iter().all(|line| line.chars().count() <= 10),
        "over-long line in {lines:?}"
    );
    // and nothing is dropped in the process
    let rejoined: String = lines.concat().chars().filter(|c| *c != ' ').collect();
    assert!(rejoined.contains(&"a".repeat(24)));
}

#[test]
fn wrap_indented_respects_the_indent_when_breaking_a_long_word() {
    let lines = wrap_indented(&"b".repeat(30), 4, 12);
    assert!(
        lines.iter().all(|line| line.chars().count() <= 12),
        "over-long line in {lines:?}"
    );
}

#[test]
fn wrap_indented_survives_a_zero_width() {
    // The layout can hand us a zero-width area mid-resize; this must not panic or hang.
    assert!(!wrap_indented("anything at all", 0, 0).is_empty());
}
