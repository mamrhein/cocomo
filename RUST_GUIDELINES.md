## Code Style and Formatting

- **MUST** use meaningful, descriptive variable and function names

- **MUST** follow [Rust API Guidelines](https://rust-lang.github.io    /api-guidelines/checklist.html) and idiomatic Rust conventions

- **MUST** use 4 spaces for indentation (never tabs)

- **NEVER** use emoji, or unicode that emulates emoji (e.g. ✓, ✗). The only exception is when writing tests and testing the impact of multibyte characters.

- Use snake\_case for functions/variables/modules, PascalCase for types/traits, SCREAMING\_SNAKE\_CASE for constants

- Limit line length to 80 characters (rustfmt default)

- **MUST** avoid including redundant comments which are tautological or self-demonstrating (e.g. cases where it is easily parsable what the code does at a glance or its function name giving sufficient information as to what the code does, so the comment does nothing other than waste user time)

- **MUST** avoid including comments which leak what this file contains, or leak the original user prompt, ESPECIALLY if it's irrelevant to the output code.

## Documentation

- **MUST** include doc comments for all public functions, structs, enums, and methods

- **MUST** document function parameters, return values, and errors

- Keep comments up-to-date with code changes

- Include examples in doc comments for complex functions

## Type System

- Prefer Types over Generics, Generics over Dyn Traits

- **MUST** leverage Rust's type system to prevent bugs at compile time

- **NEVER** use `.unwrap()` in library code; use `.expect()` only for invariant violations with a descriptive message

- **MUST** use meaningful custom error types with `thiserror` or `color-eyre`.

- Use newtypes to distinguish semantically different values of the same underlying type

- Prefer `Option\<T\>` over sentinel values

## Error Handling

- **NEVER** use `.unwrap()` in production code paths, unless a failure is impossible because of previous runtime checks. In such a case, **ALWAYS** provide a corresponding comment.

- **MUST** use `Result\<T, E\>` for fallible operations

- **MUST** use `thiserror` for defining error types and `color-eyre` for application-level errors

- **MUST** propagate errors with `?` operator where appropriate

- Provide meaningful error messages with context

## Function Design

- **MUST** keep functions focused on a single responsibility

- **MUST** prefer borrowing (`&T`, `&mut T`) over ownership when possible

- Limit function parameters to 5 or fewer; use a config struct for more

- Return early to reduce nesting

- Use iterators and combinators over explicit loops where clearer

- Use tail call functions where possible

## Struct and Enum Design

- **MUST** keep types focused on a single responsibility

- **MUST** derive common traits: `Debug`, `Clone`, `PartialEq` where appropriate

- Derive `Copy` trait where possible

- Use `\#\[derive(Default)\]` when a sensible default exists

- Prefer composition over inheritance-like patterns

- Use builder pattern for complex struct construction

- Make fields private by default; provide accessor methods when needed

## Testing

- **MUST** write unit tests for all new functions and types

- **MUST** mock external dependencies (APIs, databases, file systems)

- **MUST** use the built-in `\#\[test\]` attribute and `cargo test`

- Follow the Arrange-Act-Assert pattern

- Do not commit commented-out tests

- Use `\#\[cfg(test)\]` modules for test code

## Imports and Dependencies

- **MUST** avoid wildcard imports (`use module::\*`) except for preludes, test modules (`use super::\*`), and prelude re-exports

- **MUST** document dependencies in `Cargo.toml` with version constraints

- Use `cargo` for dependency management

- Organize imports: standard library, external crates, local modules

- Use `rustfmt` to automate import formatting

## Rust Best Practices

- **NEVER** use `unsafe` unless absolutely necessary; document safety invariants when used

- **MUST** call `.clone()` explicitly on non-`Copy` types; avoid hidden clones in closures and iterators

- **MUST** use pattern matching exhaustively; avoid catch-all `\_` patterns when possible

- **MUST** use `format!` macro for string formatting

- Use iterators and iterator adapters over manual loops

- Use `enumerate()` instead of manual counter variables

- Prefer `if let` and `while let` for single-pattern matching

## Memory and Performance

- **MUST** avoid unnecessary allocations; prefer `&str` over `String` when possible

- **MUST** use `Cow\<'\_, str\>` when ownership is conditionally needed

- Use `Vec::with\_capacity()` when the size is known

- Prefer stack allocation over heap when appropriate

- Use `Arc` and `Rc` judiciously; prefer borrowing

## Benchmarking and Optimization

- **NEVER** run benchmarks in parallel, as the benchmarks will compete for resources and the results will be invalid

- **NEVER** game the benchmarks. Do not manipulate the benchmarks themselves to satisfy any required performance constraints

- **NEVER** run benchmarks with `target-cpu=native` or any other `RUSTFLAGS`

- Ensure benchmark tests are independent. If the tests are dependent due to a feature (e.g. caching), ensure the feature is disabled

## Concurrency

- **MUST** use `Send` and `Sync` bounds appropriately

- **MUST** prefer `tokio` for async runtime in async applications

- **MUST** use `rayon` for CPU-bound parallelism

- Avoid `Mutex` when `RwLock` or lock-free alternatives are appropriate

- Use channels (`mpsc`, `crossbeam`) for message passing

## Security

- **NEVER** store secrets, API keys, or passwords in code. Only store them in `.env`

  - Ensure `.env` is declared in `.gitignore`

- **MUST** use environment variables for sensitive configuration via `dotenvy` or `std::env`

- **NEVER** log sensitive information (passwords, tokens, PII)

- Use `secrecy` crate for sensitive data types

## Version Control

- **MUST** write clear, descriptive commit messages

- **NEVER** commit commented-out code; delete it

- **NEVER** commit debug `println!` statements or `dbg!` macros

- **NEVER** commit credentials or sensitive data

## Tools

- **MUST** use `rustfmt` for code formatting

- **MUST** use `clippy` for linting and follow its suggestions

- **MUST** ensure code compiles with no warnings (use `-D warnings` flag in CI, not `\#!\[deny(warnings)\]` in source)

- Use `cargo` for building, testing, and dependency management

- Use `cargo test` for running tests

- Use `cargo doc` for generating documentation

## Before Committing

- [ ] All tests pass (`cargo test`)

- [ ] No compiler warnings (`cargo build`)

- [ ] Clippy passes (`cargo clippy -- -D warnings`)

- [ ] Code is formatted (`cargo fmt --check`)

- [ ] If the project creates a Python package and Rust code is touched, rebuild the Python package (`source .venv/bin/activate && maturin develop --release --features python`)

- [ ] If the project creates a WASM package and Rust code is touched, rebuild the WASM package (`wasm-pack build --target web --out-dir web/pkg`)

- [ ] All public items have doc comments

- [ ] No commented-out code or debug statements

- [ ] No hardcoded credentials
