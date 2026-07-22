use super::*;
use rustyline::history::DefaultHistory;

fn helper() -> NimbusHelper {
    NimbusHelper::new(Rc::new(RefCell::new(App::default())))
}

fn complete(line: &str, pos: usize) -> Vec<String> {
    let helper = helper();
    let history = DefaultHistory::new();
    let ctx = Context::new(&history);
    let (_, candidates) = helper.complete(line, pos, &ctx).unwrap();
    candidates.into_iter().map(|c| c.replacement).collect()
}

#[test]
fn completes_empty_prefix_with_all_subcommands() {
    let mut candidates = complete("", 0);
    candidates.sort();
    let mut expected: Vec<String> = SUBCOMMANDS.iter().map(|s| s.to_string()).collect();
    expected.sort();
    assert_eq!(candidates, expected);
}

#[test]
fn completes_partial_prefix() {
    let mut candidates = complete("c", 1);
    candidates.sort();
    assert_eq!(candidates, vec!["cd".to_string(), "cp".to_string()]);
}

#[test]
fn completes_unambiguous_prefix_to_single_candidate() {
    let candidates = complete("se", 2);
    assert_eq!(candidates, vec!["select".to_string()]);
}

#[test]
fn no_candidates_for_unknown_prefix() {
    let candidates = complete("zz", 2);
    assert!(candidates.is_empty());
}

#[test]
fn does_not_complete_arguments_of_other_commands() {
    let candidates = complete("put doc", 7);
    assert!(candidates.is_empty());
}

#[test]
fn does_not_complete_the_second_cd_argument() {
    let candidates = complete("cd docs sub", 11);
    assert!(candidates.is_empty());
}

#[test]
fn completes_at_cursor_position_not_end_of_line() {
    // cursor sits right after "cd", the trailing " docs" shouldn't matter
    let candidates = complete("cd docs", 2);
    assert_eq!(candidates, vec!["cd".to_string()]);
}

#[tokio::test(flavor = "multi_thread")]
async fn cd_first_argument_queries_the_app_for_completions() {
    // No vaults are registered on a default `App`, so nothing matches, but this exercises
    // the `cd`-argument code path (as opposed to the subcommand-name path above).
    let candidates = complete("cd v", 4);
    assert!(candidates.is_empty());
}
