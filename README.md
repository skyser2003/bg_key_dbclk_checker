# Keyboard chatter checker

A Rust CLI for diagnosing rapid duplicate physical keyboard presses on Windows,
macOS, and Linux. It monitors across applications without changing keyboard
behavior, ignores held-key autorepeat, and saves detected symptoms to disk.

## Install

Install Rust 1.85 or newer, then run from this repository:

```sh
cargo install --path . --locked
```

Alternatively, `cargo build --release --locked` creates the executable in
`target/release/` (`bg-key-dbclk-checker.exe` on Windows). `cargo run --` can be
used in place of the installed executable in the examples below.

## Monitor

```sh
bg-key-dbclk-checker monitor
bg-key-dbclk-checker monitor --min-ms 0 --max-ms 50
bg-key-dbclk-checker monitor --all-keys
bg-key-dbclk-checker --output ./keyboard.jsonl monitor --max-ms 80
```

The default range is **0–80 milliseconds, inclusive**. Fractional milliseconds
are accepted. A pair consists of two presses of the same physical key with a
release between them; interleaved presses of other keys do not interrupt the
comparison. Three rapid presses can produce two symptoms. Deliberately pressing
a key twice quickly can also meet this diagnostic rule.

Explicit autorepeat and duplicate key-down events while a key is held are
excluded. Keys already held when monitoring starts are excluded until released.
Timing uses native monotonic timestamps rather than wall-clock time or the
time a record is written.

Normally only symptoms are saved. `--all-keys` additionally saves every accepted
physical press, including modifiers, while using the same repeat filtering.
Release and repeat events are not saved. A press that completes a symptom creates
a `keystroke` record and a `symptom` record in this mode. The option applies only
to the current session; it does not remove records from previous sessions.

The terminal reports saved symptoms and shows the log location at startup.
Press **Ctrl+C** to stop. Capture does not install a background service.

## View and clear saved logs

```sh
bg-key-dbclk-checker logs view
bg-key-dbclk-checker logs view --kind symptom --limit 50
bg-key-dbclk-checker logs view --kind keystroke --json
bg-key-dbclk-checker --output ./keyboard.jsonl logs view --json
bg-key-dbclk-checker logs clear --yes
```

Viewing defaults to the latest **100** records of all types, in recorded order.
`--kind` selects `all`, `symptom`, or `keystroke`. Filtering happens before
limiting. `--json` returns the original JSONL rows and emits no rows for an empty
or absent log, making it suitable for scripts.

Clearing requires `--yes` and removes every record from the selected file.
Without that flag, the file is preserved and the command exits with an error.
Clearing an absent or empty file succeeds without creating it. An active monitor
can save new records after clearing finishes. Viewing and clearing work without
keyboard capture permissions.

## Storage

`--output` works with all commands and can appear before or after a subcommand.
The default file is:

| Platform | Default location |
| --- | --- |
| Windows | `%LOCALAPPDATA%\bg-key-dbclk-checker\symptoms.jsonl` |
| macOS | `~/Library/Application Support/bg-key-dbclk-checker/symptoms.jsonl` |
| Linux | `$XDG_DATA_HOME/bg-key-dbclk-checker/symptoms.jsonl`, or `~/.local/share/bg-key-dbclk-checker/symptoms.jsonl` |

Records are appended to UTF-8 JSONL, flushed, and synced to disk before a saved
notification appears. Restarting does not truncate the file. File locks
coordinate appending, viewing, and clearing; existing records remain until you
explicitly clear or remove them. New directories/files use owner-only
permissions on Unix. Write/sync failures stop monitoring with an error.

Each record has `record_type`, `schema_version` (currently `1`), `session_id`,
UTC `timestamp`, `key` (`code`, `name`), `source` (`id`, optional `name`), and
`platform`. Symptoms also include `previous_timestamp`, `interval_ms`, `min_ms`,
and `max_ms`. Key codes are native physical identifiers, not typed characters;
their numeric meanings differ across operating systems.

If a previous interrupted write left a nonempty final line without a newline,
monitoring refuses to append. Inspect/back up the file, or explicitly clear it.
Malformed records and unsupported schema versions report their line number
when viewed.

## Platform setup

### Windows

Uses background Raw Input with independent keyboard device identities and
scan-code extension flags. Ordinary software input posted with `SendInput`
does not enter this input stream. Native message timestamps have millisecond
precision, so fractional thresholds cannot create submillisecond precision.
The startup-held snapshot is global across keyboards, so an equivalent key on a
second keyboard can be conservatively excluded until released during startup.
Capture is limited to the interactive session and accessible desktop.

### macOS

Allow the executable or launching terminal in **System Settings → Privacy &
Security → Input Monitoring** (and Accessibility if requested), then restart
the program. Capture uses a listen-only event tap and includes modifier keys.
All keyboards share one source because this API does not expose keyboard device
identity. Secure input modes can prevent an event tap from seeing input.

### Linux

Uses physical `/dev/input/event*` keyboard devices directly; works with both
**X11 and Wayland**. Mouse/gamepad buttons and virtual input devices are excluded.
Newly connected accessible keyboards are discovered while monitoring.

The current user needs **read access** to the required keyboard event devices.
On a system using an `input` group, an administrator can grant membership and
you must log out and back in for it to take effect. A narrower alternative is a
device-specific udev rule granting your user read access. Use a consistent user
and `--output` location if running under a different account, so log commands
read the same file. Inaccessible devices are reported; no accessible physical
keyboard causes startup to fail. Containers/WSL may expose no usable input
devices.

Capture relies on what the operating system presents as hardware input; a driver
or device that presents synthetic input as hardware cannot be authenticated by
these APIs. A disconnect or dropped input stream resets the affected detection
history. If the bounded capture queue overflows, monitoring fails visibly rather
than silently losing input.

## Development

```sh
cargo test --locked
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
```

CI builds and tests on Linux, Windows, and macOS. Deterministic tests cover
detection, repeat/startup filtering, native key normalization, durable log
reopening, log selection, and clearing. Actual hardware capture must also be
checked on each OS by typing, holding keys, switching applications, reconnecting
keyboards, and restarting the program to verify logs persist.
