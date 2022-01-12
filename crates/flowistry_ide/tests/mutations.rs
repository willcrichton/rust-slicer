use test_env_log::test;
use utils::{find_mutations, run_tests};

mod utils;

#[test]
fn test_find_mutations() {
  run_tests("find_mutations", |path, expected| {
    find_mutations(path, expected)
  });
}
