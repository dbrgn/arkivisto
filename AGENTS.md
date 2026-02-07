# Arkivisto

Read `README.md` for a general overview.

## General

- Do not run the target binary. It will run in interactive mode and access
  physical devices. Just rely on `cargo check` and unit tests.
- If a bigger design decision is required, please ask for clarification before
  just picking an option.
- Do not remove existing block comments without good reason

## Conventions

### Rust

Imports:

- Use merged imports
- Group imports using the "std / third party / first party (`super::` / `crate::`)" convention
- Don't use `std::*` directly, instead import the corresponding modules or types at the top level
- When importing types that are only used for tests, import them inside the `tests` module and do
  not use `#[cfg(test)]` on top level
- Don't use `super::*` imports (except in test modules), instead use `crate::`
  imports

Testing:

- When adding multiple unit tests for a function, struct or enum, wrap them in a dedicated module named after
  that unit. For example, when a function is called `check_foo`, the test path should be
  `tests::check_foo::a_test` and `tests::check_foo::another_test`.
- When creating tests for complex values (i.e. complex structs, a vec containing multiple structs, etc), use
  `insta` for these tests. But when testing simple values (e.g. empty vecs, vecs of strings, etc) write
  regular unit tests without insta.
- Use `rstest` for testing multiple input-output combinations.
- When generating insta snapshots, do not accept them, let the developer review and accept manually.

Other:

- Sort dependencies (in `Cargo.toml`) and imports alphabetically
- Check if code compiles with `cargo check`
- Lint code with `cargo clippy`
- At the end, when everything else works fine, ALWAYS format code with rustfmt through `cargo fmt`
