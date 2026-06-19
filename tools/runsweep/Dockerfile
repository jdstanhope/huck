# Sandbox image for the huck runtime sweep: bash + the huck binary + the script
# corpus baked in read-only. Run with `--network none --read-only` and no host
# mounts (see run.sh) so corpus scripts are fully isolated from the host.
FROM ubuntu:24.04

# Common utilities the corpus scripts call, so "bash runs cleanly" more often.
RUN apt-get update && apt-get install -y --no-install-recommends \
        bash coreutils sed gawk mawk grep findutils util-linux procps \
        diffutils debianutils ncurses-bin file \
    && rm -rf /var/lib/apt/lists/*

# huck (release binary; same glibc as the ubuntu:24.04 host).
COPY huck /usr/local/bin/huck
RUN chmod 0755 /usr/local/bin/huck

# Corpus restored to its ORIGINAL absolute paths (so a script run by its path
# finds any sibling that is also in the corpus). Made world-readable so the
# non-root sandbox user can read every script regardless of original perms.
COPY corpus.tgz /tmp/corpus.tgz
RUN tar --no-same-owner -xzf /tmp/corpus.tgz -C / && rm /tmp/corpus.tgz

COPY paths.txt /corpus/paths.txt
RUN tr '\n' '\0' < /corpus/paths.txt | xargs -0 -r chmod a+r 2>/dev/null || true

COPY run_sweep_inner.sh /usr/local/bin/run_sweep_inner.sh
COPY triage_inner.sh /usr/local/bin/triage_inner.sh
RUN chmod 0755 /usr/local/bin/run_sweep_inner.sh /usr/local/bin/triage_inner.sh

RUN useradd -m -u 2000 sandbox
USER sandbox
WORKDIR /work
ENTRYPOINT ["/usr/local/bin/run_sweep_inner.sh"]
