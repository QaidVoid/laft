//! Tests of the recipe environment.

use crate::env::Budget;

use super::test_support::recipe;

#[test]
fn a_parallel_recipe_joins_the_jobserver() {
    let budget = Budget {
        jobs: 12,
        share: 4,
        jobserver: true,
    };
    let env = recipe("pkg").environment(budget);
    assert_eq!(env.get(crate::env::JOBS).map(String::as_str), Some("4"));
    assert_eq!(
        env.get(crate::env::MAKEFLAGS).map(String::as_str),
        Some("-j12 --jobserver-auth=fifo:/build/jobserver")
    );
}

#[test]
fn a_parallel_recipe_without_a_jobserver_names_only_its_budget() {
    let budget = Budget {
        jobs: 12,
        share: 12,
        jobserver: false,
    };
    let env = recipe("pkg").environment(budget);
    assert_eq!(env.get(crate::env::JOBS).map(String::as_str), Some("12"));
    assert_eq!(
        env.get(crate::env::MAKEFLAGS).map(String::as_str),
        Some("-j12")
    );
}

#[test]
fn a_recipe_that_does_not_build_in_parallel_gets_one_job() {
    let mut r = recipe("pkg");
    r.options.parallel = false;
    let budget = Budget {
        jobs: 12,
        share: 4,
        jobserver: true,
    };
    let env = r.environment(budget);
    assert_eq!(env.get(crate::env::JOBS).map(String::as_str), Some("1"));
    assert_eq!(
        env.get(crate::env::MAKEFLAGS).map(String::as_str),
        Some("-j1")
    );
}
