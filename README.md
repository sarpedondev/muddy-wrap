# muddy-wrap

A Rust CLI that rewrites string literals in Rust source files to use the
[`muddy`](https://crates.io/crates/muddy) obfuscation macro.

## Install

Requires Rust 1.85 or newer.

```sh
cargo install --git https://github.com/sarpedondev/muddy-wrap --locked
```

## Usage

```sh
muddy-wrap path/to/file.rs
muddy-wrap path/to/src
```

Directories are processed recursively. Files are edited in place at the
locations of the transformed literals. Existing whitespace, comments, line
endings, and unrelated source text are preserved. Review the resulting Git diff
before keeping the changes.

The destination crate needs `muddy` as a dependency:

```toml
[dependencies]
muddy = "0.3.2"
```

The rewriter adds `use muddy::muddy;`, wraps string literal expressions, and
extracts literal prefixes from supported formatting macros. It skips string
literals in `const` items, the literal `"C"`, and documentation attributes.
It also traverses blocks inside other macros.

## Development

```sh
cargo build --locked
cargo run --locked -- path/to/src
```
