# pipeline_redirect_audit_cases.sh — sourced by pipeline_redirect_audit.sh.
# Each line: <label>\t<fragment>. External stage = /bin/sh -c so a real fork
# happens (builtin-stage stderr routing is #144, deliberately excluded).
emit_cases() {
  # writer emits OUT to stdout and ERR to stderr:
  local W="/bin/sh -c 'echo OUT; echo ERR >&2'"
  cat <<EOF
ord 2>&1 >f	$W 2>&1 >pf | cat; echo --f--; cat pf
ord >f 2>&1	$W >pf 2>&1 | cat; echo --f--; cat pf
ord 2>&1	$W 2>&1 | cat
ord >f	$W >pf | cat; echo --f--; cat pf
ord 1>&2	$W 1>&2 | cat
ord >f 2>f2	$W >pf 2>pf2 | cat; echo --f--; cat pf; echo --f2--; cat pf2
dup 3>&1	/bin/sh -c 'echo THREE >&3' 3>&1 | cat
close 2>&-	$W 2>&- | cat
in <f	echo FILE > infile; /bin/cat <infile | cat
readwrite <>f	echo RW > rwfile; /bin/cat <>rwfile | cat
fd3 herestring	/bin/cat <&3 3<<<'HS' | cat
fd4 open	/bin/sh -c 'echo FOUR >&4' 4>pf | cat; echo --f--; cat pf
stage1 redir	$W 2>&1 >pf | cat; echo --f--; cat pf
last redir	/bin/echo A | /bin/sh -c 'cat; echo ERR >&2' 2>&1 >pf; echo --f--; cat pf
capture ctx	out=\$($W 2>&1 >pf | cat); echo "cap=[\$out]"; echo --f--; cat pf
EOF
}
