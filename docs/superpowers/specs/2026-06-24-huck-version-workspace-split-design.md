# v213: HUCK_VERSION across workspace split + `huck --version` flag

## Goal

Fix `$HUCK_VERSION` to reflect the user-facing release version (the root
`huck` package's `CARGO_PKG_VERSION`), not the internal `huck-engine`
crate's version. Add a `huck --version` / `-V` CLI flag that prints the
same value. Restore the strict equality assertion in
`tests/builtin_vars.rs::bash_version_and_huck_version` that v212 had to
loosen because of the workspace-split skew.

## Background

The v202 huck-syntax extraction (merge `5531dbe`, 2026-06-22) split the
runtime crate into a workspace, and v203 (merge `0f9778d`) extracted
`huck-engine`. Both internal-only crates kept their version pinned at
`0.1.0` per the `docs/RELEASING.md` policy ("workspace member version is
internal and independent; the root `huck` `Cargo.toml` is the single
source of release-version truth").

However, `crates/huck-engine/src/shell_state.rs:1981` continued setting
`$HUCK_VERSION = env!("CARGO_PKG_VERSION")` — which resolves at compile
time to `huck-engine`'s own version (`0.1.0`), not the root `huck`
crate's release version (`0.2.0` at v0.2.0 release, `0.3.0-dev` today).
Users running a freshly-released `huck` binary see `$HUCK_VERSION` stuck
at `0.1.0` indefinitely.

The skew surfaced in v212's full-sweep verification: the integration
test `tests/builtin_vars.rs::bash_version_and_huck_version` was
asserting `$HUCK_VERSION == env!("CARGO_PKG_VERSION")` from the
test-crate's POV (`0.3.0-dev` — the test crate IS the root `huck`
crate), which failed against the engine's `0.1.0`. v212 (commit
`2f86197`) loosened the assertion to "non-empty digit-led string" as a
holding pattern and deferred the deeper fix here.

Additionally, `huck --version` / `-V` is unimplemented today — the CLI
rejects both flags with `unrecognized option`. Bash supports
`--version`; users expect huck to do the same.

## Scope

**In scope:**

- Plumb the root-crate release version through the CLI layer to
  `huck-engine`:
  - `src/main.rs` passes `env!("CARGO_PKG_VERSION")` (root crate's
    value) to a new `huck_cli::run(&args, version: &str) -> i32`.
  - `huck_cli::run` threads the version into the shell-cell via
    `shell.set("HUCK_VERSION", v)` at startup, BEFORE any RunMode
    dispatch.
- Add `EngineBuilder::with_version(&str)` so external embedders can
  set the same value.
- Add `huck --version` / `-V` flag:
  - New `RunMode::PrintVersion` variant in `crates/huck-engine/src/shell.rs`.
  - `parse_cli` matches `--version` / `-V` and returns `RunMode::PrintVersion`.
  - `repl::run` handles the variant by printing `huck {version}\n` to
    stdout and returning 0.
- Restore the strict equality assertion in
  `tests/builtin_vars.rs::bash_version_and_huck_version`:
  - `assert_eq!(huck("echo $HUCK_VERSION").trim(), env!("CARGO_PKG_VERSION"));`
  - Remove the temporary "non-empty digit-led" workaround + comment.
- Add 4 unit tests:
  - `parse_cli_version_long` — `parse_cli(&["--version".into()])` →
    `RunMode::PrintVersion`.
  - `parse_cli_version_short` — `parse_cli(&["-V".into()])` →
    `RunMode::PrintVersion`.
  - `builder_with_version_sets_huck_version` — `Engine::builder().with_version("9.9.9").build()`
    then run `echo $HUCK_VERSION`, assert `"9.9.9"`.
  - `version_round_trips_through_run` — call `huck_cli::run` with a
    `--version` arg and a synthetic version string, capture stdout, assert
    `"huck X.Y.Z\n"`. (Optional — drop if it's awkward without
    intercepting stdout.)

**Out of scope:**

- Multi-line version output (bash prints copyright + build metadata; we
  emit just `huck X.Y.Z`).
- A `--help` flag (separate iteration if wanted).
- Bumping `huck-engine`'s own `Cargo.toml` version. Per
  `docs/RELEASING.md`, workspace member versions stay internal.
- Bash-divergence catalog updates. This isn't a bash-divergence — it
  was an internal workspace-split bug introduced post-v202.

## Architecture

### Source-of-truth flow

```
src/main.rs (root huck crate; version = "0.3.0-dev" today, bumped on each release)
  └── env!("CARGO_PKG_VERSION") → passed to huck_cli::run

crates/huck-cli/src/lib.rs / repl.rs:
  pub fn run(args: &[String], version: &str) -> i32 {
      let opts = parse_cli(args)?;
      if RunMode::PrintVersion: println!("huck {version}"); return 0;
      // else: set $HUCK_VERSION on shell_cell, then proceed normally
      shell_cell.borrow_mut().set("HUCK_VERSION", version.to_string());
      match opts.mode { RunMode::Command {...} | RunMode::File {...} | RunMode::Interactive => ... }
  }

crates/huck-engine/src/engine.rs::EngineBuilder:
  pub fn with_version(self, v: &str) -> Self { self.version = Some(v.into()); self }
  // build() applies via e.set_var("HUCK_VERSION", v)

crates/huck-engine/src/shell_state.rs:1981:
  // UNCHANGED. Default still env!("CARGO_PKG_VERSION") of huck-engine.
  // This is the FALLBACK when no embedder overrides — applies to
  // unit tests inside huck-engine, and to embedders that don't call
  // .with_version().
  self.install_var("HUCK_VERSION", env!("CARGO_PKG_VERSION").to_string(), false);
```

The huck-engine default at `shell_state.rs:1981` stays put. CLI and
external embedders OVERRIDE via `shell.set` (or
`EngineBuilder::with_version → set_var → set`).

### Why not bake the version into huck-engine via build script

Tempting alternative: add a `build.rs` to the root crate that writes a
const file consumed by huck-engine. This would avoid the
`huck_cli::run` signature change. Rejected because:

- Cross-crate `build.rs` value injection is awkward and fragile (env
  vars, `OUT_DIR`, `include!`).
- The embedding-arc design (v204-v211) explicitly treats huck-engine
  as a clean library that doesn't know who's embedding it. The
  embedder telling the engine "I'm version X" is the right contract.
- External embedders (any future huck-engine consumer) get the same
  ergonomic API as the CLI.

### Why not env var injection

Considered: huck-engine reads `HUCK_VERSION_OVERRIDE` env var if set.
Rejected because:

- Introduces hidden runtime coupling.
- The Engine builder API is already the documented embedder surface;
  adding a hidden env override muddles it.

### Breaking signature change

`huck_cli::run(&[String]) -> i32` becomes `huck_cli::run(&[String],
&str) -> i32`. This is a public API change.

**Known consumers (1):** `src/main.rs` only. Ripple is one line.

External consumers of `huck-cli` as a library are not expected; the
crate is documented as huck's REPL+rustyline adapter. v211 polished
huck-syntax for external publication; huck-cli was never on that
trajectory. Acceptable breaking change.

## Behavioral / observable changes

**Before:**
- `huck -c 'echo $HUCK_VERSION'` → `0.1.0` (always; huck-engine's
  pinned version).
- `huck --version` → `huck: unrecognized option: --version`, exit 2.
- `huck -V` → `huck: unrecognized option: -V`, exit 2.

**After:**
- `huck -c 'echo $HUCK_VERSION'` → `0.3.0-dev` (today; whatever
  `version` field is in the root `Cargo.toml` at build time).
- `huck --version` → `huck 0.3.0-dev`, exit 0.
- `huck -V` → `huck 0.3.0-dev`, exit 0.

**Unaffected:**
- Users can still overwrite `HUCK_VERSION` at runtime (not readonly).
  Matches `BASH_VERSION`.
- Embedded Engine without `.with_version()`: gets huck-engine's
  `0.1.0` default (correct — they're embedding the library).
- Engine builder paths (`from_shell_cell`, manual `set_var` after
  construction): unchanged.

## Testing strategy

### Unit tests (4 new, 1 restored)

**`crates/huck-engine/src/shell.rs::mod tests`** (2 new):

```rust
#[test]
fn parse_cli_version_long() {
    let opts = parse_cli(&["--version".to_string()]).expect("parse");
    assert!(matches!(opts.mode, RunMode::PrintVersion));
}

#[test]
fn parse_cli_version_short() {
    let opts = parse_cli(&["-V".to_string()]).expect("parse");
    assert!(matches!(opts.mode, RunMode::PrintVersion));
}
```

**`crates/huck-engine/src/engine.rs::mod tests`** (1 new):

```rust
#[test]
fn builder_with_version_sets_huck_version() {
    let mut e = Engine::builder().with_version("9.9.9").build();
    let out = e.capture("echo $HUCK_VERSION");
    assert_eq!(out.stdout.trim(), "9.9.9");
    assert_eq!(out.exit_code, 0);
}
```

(If the existing capture method has a different name — `e.exec(...).capture()` perhaps — adapt to it. The point is to verify `$HUCK_VERSION` reflects the builder-provided value.)

**`tests/builtin_vars.rs::bash_version_and_huck_version`** (1 restored):

```rust
#[test]
fn bash_version_and_huck_version() {
    assert_eq!(huck("[ -n \"$BASH_VERSION\" ] && echo yes").trim(), "yes");
    assert_eq!(huck("echo ${BASH_VERSINFO[0]}").trim(), "5");
    assert_eq!(huck("echo $HUCK_VERSION").trim(), env!("CARGO_PKG_VERSION"));
}
```

Drop the v212 loosening comment + the non-empty/digit-led fallback. The
test-crate's `CARGO_PKG_VERSION` IS the root `huck` crate's version
(`tests/` lives under the root crate), so the assertion is now correct.

### No bash-diff harness

The `huck --version` flag is huck-specific output (bash's multi-line
`bash --version` carries GNU build metadata we can't match). Not a
candidate for bash-diff.

### Existing harness regression

No expected impact. Spot-check a release build + `huck --version`
manually after implementation.

## Documentation updates

- `docs/architecture.md`: one-sentence note in the "where to add common
  features" cheatsheet (if it exists) for "CLI version-stamped variables"
  pointing at `huck_cli::run` + `EngineBuilder::with_version`.
- No `docs/bash-divergences.md` change.

## Acceptance

- `huck -c 'echo $HUCK_VERSION'` reports the root `Cargo.toml`
  `version` value.
- `huck --version` and `huck -V` both print `huck $VERSION\n` and exit
  0.
- `tests/builtin_vars.rs::bash_version_and_huck_version` strict
  assertion restored and passing.
- 4 new unit tests pass.
- `cargo test --workspace` green.
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- `cargo build --release --workspace` clean.
- ALL 131 existing `*_diff_check.sh` harnesses still pass.
