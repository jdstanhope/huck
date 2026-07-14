# bg_pipeline_redirect_audit_cases.sh — sourced by bg_pipeline_redirect_audit.sh.
# Each line: <label>\t<space-separated result files>\t<fragment>.
# Fragment is a BARE `pipeline &` (rest empty => run_background_sequence). External
# stage = /bin/sh -c so a real fork happens. The consumer stage captures the piped
# stream to a file so it is observable after the detached job finishes.
emit_bg_cases() {
  local W="/bin/sh -c 'echo O; echo E >&2'"
  # NOTE: real tab characters separate the three fields below.
  printf '%s\t%s\t%s\n' "ord 2>&1 >f"   "pf po" "$W 2>&1 >pf | cat >po &"
  printf '%s\t%s\t%s\n' "ord >f 2>&1"   "pf po" "$W >pf 2>&1 | cat >po &"
  printf '%s\t%s\t%s\n' "ord >f 2>f2"   "pf pf2 po" "$W >pf 2>pf2 | cat >po &"
  printf '%s\t%s\t%s\n' "fd4 open"       "pf po" "/bin/sh -c 'echo FOUR >&4' 4>pf | cat >po &"
  printf '%s\t%s\t%s\n' "fd3 dup"        "po" "/bin/sh -c 'echo THREE >&3' 3>&1 | cat >po &"
  printf '%s\t%s\t%s\n' "in <f"          "po" "echo FILE > infile; /bin/cat <infile | cat >po &"
  printf '%s\t%s\t%s\n' "stage0 nodir"   "po" "/bin/cat | cat >po &"
  printf '%s\t%s\t%s\n' "close 2>&-"     "po" "$W 2>&- | cat >po &"
  printf '%s\t%s\t%s\n' "last redir"     "pf po" "/bin/echo A | /bin/sh -c 'cat; echo E >&2' 2>&1 >pf | cat >po &"
}
