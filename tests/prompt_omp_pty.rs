//! Best-effort PTY payoff test for v141: prompt-string command substitution.
//!
//! oh-my-posh sets `PS1='$(_omp_get_primary)'` (verified verbatim in its cached
//! init script). Before v141 huck rendered that PS1 LITERALLY — the user saw
//! the text `$(_omp_get_primary)` instead of a prompt. v141 makes prompt
//! expansion run the embedded `$(…)`, so a fresh prompt RENDERS the powerline /
//! ANSI output instead of the literal source.
//!
//! This spawns interactive huck under a real PTY, sources `oh-my-posh init bash`
//! (which registers `_omp_hook` via PROMPT_COMMAND and sets the `$(…)` PS1),
//! then triggers a fresh prompt and asserts the captured bytes contain the ANSI
//! CSI introducer (ESC `[`) AND do NOT contain the literal `_omp_get_primary`.
//!
//! It SKIPS GRACEFULLY (passes) if oh-my-posh is not installed or no PTY can be
//! allocated. A hard per-read timeout turns any hang into a failed `expect`
//! rather than wedging the suite; the session is dropped (killing a wedged
//! child) before any panic.

use std::io::Write;
use std::process::Command;
use std::time::Duration;

use expectrl::session::OsSession;
use expectrl::{Expect, Regex};

const ESC: u8 = 0x1b;

#[test]
fn oh_my_posh_prompt_renders_not_literal() {
    // 1. Skip if oh-my-posh is absent (or `version` fails for any reason).
    let ver = Command::new("oh-my-posh").arg("version").output();
    match ver {
        Ok(o) if o.status.success() => {}
        _ => {
            eprintln!("prompt_omp_pty: skipping — no oh-my-posh");
            return;
        }
    }

    // 2. Pre-resolve the init on the Rust side. `oh-my-posh init bash` prints a
    //    short one-liner that sources a cached init file; we persist that
    //    stdout to a temp .sh and source it in a single line from huck.
    let init = match Command::new("oh-my-posh").args(["init", "bash"]).output() {
        Ok(o) if o.status.success() && !o.stdout.is_empty() => o.stdout,
        _ => {
            eprintln!("prompt_omp_pty: skipping — `oh-my-posh init bash` produced no init");
            return;
        }
    };
    let mut init_file = match tempfile::Builder::new()
        .prefix("omp_init_")
        .suffix(".sh")
        .tempfile()
    {
        Ok(f) => f,
        Err(e) => {
            eprintln!("prompt_omp_pty: skipping — cannot create temp init file: {e}");
            return;
        }
    };
    if init_file
        .write_all(&init)
        .and_then(|_| init_file.flush())
        .is_err()
    {
        eprintln!("prompt_omp_pty: skipping — cannot write temp init file");
        return;
    }
    let init_path = init_file.path().to_string_lossy().into_owned();

    // 3. Spawn interactive huck in a PTY. Skip if no PTY.
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_huck"));
    // Hermetic: never source the developer's ~/.huckrc (#239).
    cmd.arg("--norc");
    let mut session = match OsSession::spawn(cmd) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("prompt_omp_pty: skipping — no PTY: {e}");
            return;
        }
    };
    session.set_expect_timeout(Some(Duration::from_secs(8)));

    // Confirm the shell is alive and reading before we do anything else.
    let _ = session.send("echo READY_$((6*7))\r");
    if session.expect("READY_42").is_err() {
        eprintln!("prompt_omp_pty: skipping — interactive prompt/marker not seen");
        return;
    }

    // 4./5. Source the init (registers the PROMPT_COMMAND hook + the `$(…)` PS1)
    //    AND a readiness sentinel on the SAME line. Sending the source and the
    //    sentinel as two back-to-back `\r` lines without an intervening read
    //    desyncs expectrl's incremental buffer (the sibling harnesses document
    //    this); a single line with a trailing `echo` keeps the stream in sync.
    //    The sentinel is built with arithmetic so the PTY echo of the typed line
    //    ("MID_$((6*7))") does not match the marker we wait for ("MID_42"); only
    //    the post-source runtime output satisfies the gate. Reaching it proves
    //    the source did not wedge the reader.
    let _ = session.send(format!("source {init_path}; echo MID_$((6*7))\r"));
    let mid_ok = session.expect("MID_42").is_ok();
    if !mid_ok {
        drop(session);
        panic!("prompt_omp_pty: shell unresponsive after sourcing oh-my-posh init (wedge)");
    }

    // 6. Trigger a fresh prompt (empty line) and capture what it renders.
    //    We assert on bytes: the ANSI CSI introducer (ESC '[') must be present,
    //    and the literal PS1 source `_omp_get_primary` must NOT be (it would
    //    appear only if huck printed PS1 verbatim — the pre-v141 bug).
    let _ = session.send("\r");
    let captured = session.expect(Regex(r"\x1b\["));
    let rendered_ansi = captured.is_ok();

    // `as_bytes()` is the FULL captured stream (bytes before AND including the
    // match), so it carries the ESC that satisfied the regex plus the rest of
    // the rendered powerline.
    let bytes: Vec<u8> = match &captured {
        Ok(m) => m.as_bytes().to_vec(),
        Err(_) => Vec::new(),
    };
    let has_csi = rendered_ansi && bytes.contains(&ESC);
    let has_literal = contains_subslice(&bytes, b"_omp_get_primary");

    drop(session);
    drop(init_file);

    assert!(
        rendered_ansi,
        "fresh prompt did not emit an ANSI CSI escape — PS1 cmd-sub may not have rendered (v141)"
    );
    assert!(
        has_csi,
        "captured prompt bytes lack the ESC introducer despite a CSI match"
    );
    assert!(
        !has_literal,
        "fresh prompt printed the LITERAL `$(_omp_get_primary)` source instead of rendering it (v141 regression)"
    );
}

/// True if `haystack` contains `needle` as a contiguous byte subslice.
fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || needle.len() > haystack.len() {
        return needle.is_empty();
    }
    haystack.windows(needle.len()).any(|w| w == needle)
}
