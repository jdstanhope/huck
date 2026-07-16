//! Regression test for #178: the `exec N>…` / `exec N>&-` permanent-redirect
//! path must not leak heap. `apply_redirects_permanently` used to
//! `mem::forget` its `RedirectScope`, leaking the scope's Vec buffers (~100
//! bytes) on every `exec` redirect — unbounded over a long-running process.
//!
//! In its OWN integration binary (its own process) so the RSS measurement isn't
//! confounded by other tests' allocations. Linux-only (reads `/proc/self`).

use huck_engine::Engine;

#[cfg(target_os = "linux")]
fn rss_kb() -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").expect("read /proc/self/status");
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            // "VmRSS:   12345 kB"
            let kb = rest.split_whitespace().next().unwrap_or("0");
            return kb.parse().unwrap_or(0);
        }
    }
    0
}

/// Run `count` `exec 8>/dev/null; exec 8>&-` cycles in a single shell.
fn run_exec_redirects(e: &mut Engine, count: u32) {
    let script =
        format!("i=0; while [ $i -lt {count} ]; do exec 8>/dev/null; exec 8>&-; i=$((i+1)); done");
    let rc = e.run(&script);
    assert_eq!(rc, 0, "exec-redirect loop should exit 0");
}

#[test]
#[cfg(target_os = "linux")]
fn exec_redirect_does_not_leak() {
    let mut e = Engine::new();

    // Warm up so the allocator reaches steady state and pages are faulted in;
    // the baseline is taken AFTER warmup so we measure only sustained growth.
    run_exec_redirects(&mut e, 10_000);
    let before = rss_kb();

    // The measured run. With the leak (~100 B/iter) this adds ~6 MB; with the
    // fix the freed scope is reused each iteration and RSS stays flat.
    run_exec_redirects(&mut e, 60_000);
    let after = rss_kb();

    let growth = after.saturating_sub(before);
    assert!(
        growth < 2_500,
        "exec-redirect RSS grew {growth} KB over 60k iterations (before={before} after={after}); \
         a leak in apply_redirects_permanently regressed (#178) — expected < 2500 KB"
    );
}

// On non-Linux the /proc-based measurement isn't available; keep the binary
// buildable with a trivial always-pass test.
#[test]
#[cfg(not(target_os = "linux"))]
fn exec_redirect_does_not_leak() {
    let mut e = Engine::new();
    run_exec_redirects(&mut e, 1_000);
}
