# Keyboard chatter detector implementation plan

> Implement in this session using executing-plans and test-driven-development.

**Goal:** Implement the approved cross-platform keyboard diagnostic CLI with durable
symptom storage, optional physical-press logging, and log viewing/clearing.

**Architecture:** Native capture adapters feed normalized, timestamped events to a
bounded channel. A shared detector ignores repeat and held-key events. Locked
JSONL storage syncs each record; log commands operate without starting capture.

**Tech stack:** Rust, clap, serde/serde_json, chrono, dirs, fs2, ctrlc, evdev,
Windows Raw Input through windows-sys, macOS CoreGraphics/CoreFoundation FFI.

**Spec:** ../specs/2026-10-11-keyboard-chatter-design.md

## Constraints and review focus

- Inclusive default interval: 0–80 ms; finite nonnegative bounds; maximum > 0.
- Physical key identity is independent of character case/layout. Repeat events,
  held-key downs, and startup-held keys never become keystroke or symptom records.
- Logs append, flush, and sync; clear requires `--yes`; viewing defaults to the
  latest 100 matching records and does not create missing files.
- Input gaps reset detection; bounded-channel overflow stops capture visibly.
- Windows, macOS, and Linux adapters must compile. Real hardware testing needs
  the corresponding operating systems and input devices.
- Review focus: concurrent clearing/appending, stale startup-held keys, input
  gaps, incorrect monotonic timestamps, and capture shutdown without input.

## Task 1: shared events and detector

Files: Cargo.toml, src/lib.rs, src/event.rs, src/detector.rs, tests/detector.rs.
Interfaces: `KeyEvent` carries source, key, action, monotonic ns, and UTC timestamp;
`Detector::process` returns accepted press plus optional symptom; reset seeds held
keys for a source and clears timing history.

- [x] Write deterministic tests for bounds, repeat filtering, interleaved keys,
      separate devices, triples, monotonic time, resets, and invalid ranges.
- [x] Run tests and confirm the missing behavior fails.
- [x] Implement the minimal event model and detector; run the full suite.
- [x] Commit the independently tested shared logic.

## Task 2: persistent storage and log queries

Files: src/record.rs, src/storage.rs, src/session.rs, tests/storage.rs.
Interfaces: tagged `LogRecord`; `LogStore::open/append`, `view(path, kind, limit)`
returning original JSON lines plus parsed records, `clear(path, confirmed)`;
`Session::process` converts accepted events to persisted records.

- [x] Write real-filesystem tests for append/reopen, all-keys vs symptom mode,
      filtering-before-limiting, malformed/schema errors, incomplete trailing
      lines, clear confirmation, absent logs, and appending after clear.
- [x] Confirm tests fail, implement locking and durable writes, then run all tests.
- [x] Add concurrent append/read/clear checks and injected write-error coverage.
- [x] Commit the verified persistence and session logic.

## Task 3: native platform adapters

Files: src/capture/mod.rs, src/capture/linux.rs, src/capture/windows.rs,
src/capture/macos.rs, platform-specific normalization tests.
Interfaces: `run(SyncSender<CaptureMessage>, Arc<AtomicBool>) -> anyhow::Result<()>`;
messages are `Event(KeyEvent)` or `Reset { source, held_keys }`.

- [x] Add tests for native repeat classification and physical key identity.
- [x] Implement Linux evdev, Windows Raw Input, and macOS listen-only event tap.
      Platform files can be developed independently against the fixed interface.
- [x] Seed held keys at startup; reset on reconnect/drop; report permissions.
- [x] Timestamp before queueing; fail rather than silently lose events on overflow.
- [x] Verify platform compilation and shutdown/error paths available on the host.

## Task 4: CLI, documentation, CI, and review

Files: src/cli.rs, src/main.rs, tests/cli.rs, README.md, .gitignore,
.github/workflows/ci.yml.

- [x] Write CLI integration tests for help, range/limit validation, default/global
      paths, view/clear semantics, and log commands without capture permissions.
- [x] Implement commands, Ctrl+C shutdown, capture worker integration, and status.
- [x] Run cargo fmt --check, cargo test, cargo clippy --all-targets -- -D warnings,
      and cargo check for Windows/macOS targets.
- [x] Document build/install, permissions, commands, JSON schema, and limitations.
- [x] Review the full change, fix material findings, run required checks, commit.

## Execution record

The user approved implementation of the revised specification. Execute the plan
without another approval round. Work on the existing `work` branch in this managed
workspace. Platform adapter work is independent; use the parallel-agents skill
for those files and a separate reviewer at the end. No publishing or remote push
is part of this task.

Verification: 49 tests pass; formatting and Linux/Windows/macOS all-target lint
and compilation checks pass; Linux release build succeeds. Independent review
findings were fixed with failing-then-passing regression tests. Native hardware
capture remains a manual check on each operating system.
