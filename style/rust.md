# Rust Style Guide

The canonical coding standard for the `src-tauri/` backend (Tauri 2.0 + Rust).
`AGENTS.md` links here instead of duplicating these rules — this file is the
single source of truth.

Every rule lists its **enforcing tool**:

| Tag | Meaning |
|-----|---------|
| `fmt` | `cargo fmt --check` (config in `rustfmt.toml`) |
| `clippy` | `cargo clippy --all-targets --all-features -- -D warnings` (config in `clippy.toml`) |
| `compiler` | `rustc` / exhaustiveness — a violation fails to compile |
| `review` | No automated check; enforced by human/agent review |

Run `cd src-tauri && cargo fmt --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test`
before committing larger backend changes. The lefthook pre-push hook runs these
checks after the frontend checks.

---

## 1. Formatting & lint hygiene

- **`cargo fmt` and `cargo clippy --all-targets` must pass.** No exceptions. — `fmt` / `clippy`
- **No `#[allow(...)]` without a one-line comment** justifying why. — `review`
- **`snake_case` everywhere.** Modules small and focused; split a file approaching 800 LOC. — `clippy` (naming) / `review` (size)

## 2. Error handling — no silent fallbacks

- **Propagate with `?`.** Wrap cross-layer errors with `anyhow::Context`; use `thiserror`-derived typed enums for errors crossing module boundaries. Never bubble a bare `String` error. — `review`
- **No `unwrap()` / `expect()` outside `#[cfg(test)]`.** Tests may use them freely. — `clippy` (`clippy::unwrap_used` / `clippy::expect_used`, warn; `allow-unwrap-in-tests = true`)
- **No plausible-but-wrong substitutes.** Never use a parent/session value where a per-record value is required, write `None`/placeholder where a real value should be computed, use non-deterministic `HashMap::iter().find_map()` as a lookup, or `unwrap_or_default()` to mask missing data. If the correct value is unobtainable, **log a warning and skip** — do not fabricate. — `review`
- **Log levels:** `log::warn!` when skipping a record, `log::error!` for recovered I/O failures, `log::debug!` for parser internals. — `review`
- **Never `eprintln!` in production paths.** — `review` (a clippy `disallowed_macros` ban can't exempt tests or Tauri's generated code, so this stays review-enforced)

```rust
// ✅
let value = row.get(idx).context("usage row missing token_count")?;
// ❌
let value = row.get(idx).unwrap_or_default();
```

## 3. Parsers

- **Malformed JSONL line / field → log a warning with file path + line context, then skip.** Never silently produce partial/empty results. — `review`
- **Do not use truncated summaries (`compact_string`) for matching/comparison** — extract full values from source JSON. — `review`
- **No heuristic substring/UUID scans where a structured signal exists.** Use each provider's typed parent/child field. — `review`

## 4. Design & idioms

- **Accept interfaces, return structs.** Keep traits small (1–3 methods). — `review`
- **Match arms exhaustive.** No `_ => unreachable!()` for the `Provider` enum or other internal enums — adding a variant must force every match to be revisited (the compile error is the feature). — `compiler` / `review`
- **No helpers used exactly once** — inline them. No premature cross-provider abstraction when fewer than 3 providers actually share the shape. — `review`
- **No `COALESCE(excluded.x, sessions.x)` for parser-authoritative fields** — only for genuinely back-filled fields where multiple sync passes converge. — `review`
- **Functions under ~50 lines, nesting under 4 levels.** Extract per-case handlers; use guard clauses / `let … else`. — `clippy` (`cognitive-complexity-threshold`, `too-many-arguments-threshold`) / `review`

## 5. Security & trust boundaries

- **Tauri commands are a trust boundary** — validate inputs (`Provider::parse_strict`, canonicalize `PathBuf` args). Don't `unwrap()` user-supplied strings. — `review`
- **No `unsafe`** without a comment block explaining the invariant upheld and what breaks if violated. — `review`
- **No secrets** in code or fixtures — no API keys, auth tokens, or real session IDs tied to a person. — `review`
- **`tauri.conf.json` asset scope is allowlist-only.** A new provider adds its specific subtree, never `$HOME/**`. — `review`

## 6. Testing

- **Unit tests in `#[cfg(test)] mod tests`** at file bottom; cross-file tests in `src-tauri/tests/<area>.rs`. — `review`
- **Test naming:** `<unit>_<scenario>_<expected>` (e.g. `parent_backfills_child_when_parser_declares_child_ids`). — `review`
- **Golden fixtures** in `src-tauri/tests/fixtures/<provider>/` for parser regression; synthetic in-test JSON for behavioral edge cases. — `review`
- **Test data must be synthetic** — placeholder UUIDs like `11111111-1111-4111-a111-111111111111`, never real session IDs/usernames/paths. — `review`
- **Real-data smoke tests** that read `~/.<provider>/` MUST be `#[ignore]` and assert structural invariants only. — `review`
- **Every bug fix adds a regression test** — paste the original bad input as a fixture. — `review`

## 7. Adding a `Provider` variant

Update all of: `models.rs::Provider`, `provider.rs::PROVIDER_CATALOG` + `provider_entry` match,
`tauri.conf.json` scope, `src/lib/types.ts`, `src/styles/variables.css`,
`src/stores/providerSnapshots.ts` fallback. The compile errors list most but not all. — `compiler` (partial) / `review`

---

### Quick checklist before commit

- [ ] `cargo fmt --check` clean
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` clean
- [ ] `cargo test` green (incl. golden fixtures)
- [ ] No `unwrap`/`expect`/`eprintln!` in production paths
- [ ] Errors propagated with `?` + context; no silent `None`/default fallbacks
- [ ] Match arms exhaustive; no single-use helpers
- [ ] New behavior / bug fix has a synthetic-data test
