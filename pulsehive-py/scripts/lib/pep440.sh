# pep440.sh — the release path's one PEP 440 version rule (spine r2.s2 review,
# class B). Sourced by py-wheel-smoke.sh and py-publish-check.sh; never run on
# its own.
#
# The expected version is the literal `version` under [package] in
# pulsehive-py/Cargo.toml (and, at the publish gate, the `v*` tag's spelling of
# it), while maturin writes the wheel's version in PEP 440's canonical
# spelling: Cargo's `3.0.0-beta.1` is `3.0.0b1` on the wheel — in its filename,
# in its METADATA and in the metadata of the installed distribution. Comparing
# the two byte-for-byte therefore rejects every correctly-built prerelease
# (and would let a tag's spelling disagree with the artifact the same version
# names). Both sides of every version comparison on the release path pass
# through the rule here first, so the spelling a version is written in never
# decides whether it is the same version.
#
#   version_eq <a> <b>       rc 0 iff a and b are the same PEP 440 version
#   pep440_canon <version>   the canonical spelling on stdout; rc non-zero when
#                            the value has no release segment to canonicalize
#                            (callers treat that as "not comparable")
#
# The rule is PEP 440's own normalization: case folding, an optional leading
# `v`, `-`/`_`/`.` as interchangeable separators, alpha/a, beta/b, c/pre/
# preview/rc, post/rev/r and dev spellings, `-N` as an implicit post-release,
# local-version separators, and release groups compared numerically so that
# `1.0` and `1.0.0` are the same version (as they are to pip and PyPI).

# The parser is awk (POSIX), so this rule costs the release path no interpreter
# beyond the ones it already runs — the smoke is invoked on macOS, Linux and
# the Windows runner's Git Bash, and all three ship awk.
PEP440_AWK='
function trim(s) { gsub(/^[ \t\r\n]+/, "", s); gsub(/[ \t\r\n]+$/, "", s); return s }
function strip0(s) { sub(/^0+/, "", s); return (s == "" ? "0" : s) }
function tagof(m) { sub(/[0-9]+$/, "", m); gsub(/[-_.]/, "", m); return m }
function numof(m) { sub(/^[^0-9]*/, "", m); return (m == "" ? "0" : strip0(m)) }

# canon(v): canonical spelling of v, or "" when v is not a PEP 440-shaped value
function canon(v,   s, rel, rest, out, i, n, m, tag, rg, nrel, loc) {
  s = tolower(trim(v))
  sub(/^v/, "", s)
  if (!match(s, /^[0-9]+(\.[0-9]+)*/)) return ""
  rel = substr(s, 1, RLENGTH)
  rest = substr(s, RLENGTH + 1)
  nrel = split(rel, rg, ".")
  out = strip0(rg[1])
  for (i = 2; i <= nrel; i++) out = out "." strip0(rg[i])
  if (match(rest, /^[-_.]?(alpha|beta|preview|pre|rc|a|b|c)[-_.]?[0-9]*/)) {
    m = substr(rest, 1, RLENGTH); rest = substr(rest, RLENGTH + 1)
    tag = tagof(m)
    if (tag == "alpha") tag = "a"
    else if (tag == "beta") tag = "b"
    else if (tag == "c" || tag == "pre" || tag == "preview") tag = "rc"
    out = out tag numof(m)
  }
  if (match(rest, /^[-_.]?(post|rev|r)[-_.]?[0-9]*/)) {
    m = substr(rest, 1, RLENGTH); rest = substr(rest, RLENGTH + 1)
    out = out ".post" numof(m)
  } else if (match(rest, /^-[0-9]+/)) {
    m = substr(rest, 1, RLENGTH); rest = substr(rest, RLENGTH + 1)
    out = out ".post" strip0(substr(m, 2))
  }
  if (match(rest, /^[-_.]?dev[-_.]?[0-9]*/)) {
    m = substr(rest, 1, RLENGTH); rest = substr(rest, RLENGTH + 1)
    out = out ".dev" numof(m)
  }
  if (match(rest, /^\+[a-z0-9]+([-_.][a-z0-9]+)*/)) {
    loc = substr(rest, 1, RLENGTH); rest = substr(rest, RLENGTH + 1)
    sub(/^\+/, "", loc)
    gsub(/[-_.]/, ".", loc)
    out = out "+" loc
  }
  if (rest != "") return ""
  return out
}

# relsplit(c): split a canonical form into release and suffix, both in globals.
# The dot a canonical post/dev suffix carries is a suffix separator, not a
# release-group separator, so it is not part of the release.
function relsplit(c,   i, ch, parts) {
  for (i = 1; i <= length(c); i++) {
    ch = substr(c, i, 1)
    if (ch != "." && (ch < "0" || ch > "9")) break
  }
  G_REL = substr(c, 1, i - 1)
  sub(/\.$/, "", G_REL)
  G_SUF = substr(c, i)
  return split(G_REL, parts, ".")
}

# padrel(rel, w): the release groups, short ones padded with trailing zeros
function padrel(rel, w,   n, parts, i, out) {
  n = split(rel, parts, ".")
  out = ""
  for (i = 1; i <= w; i++) out = out (i == 1 ? "" : ".") (i <= n ? parts[i] : "0")
  return out
}

BEGIN {
  if (mode == "canon") {
    if ((c = canon(v)) == "") exit 2
    print c
    exit 0
  }
  if (a == b) { print 1; exit 0 }
  ca = canon(a); cb = canon(b)
  if (ca == "" || cb == "") { print 0; exit 0 }
  na = relsplit(ca); ra = G_REL; sa = G_SUF
  nb = relsplit(cb); rb = G_REL; sb = G_SUF
  w = (na > nb) ? na : nb
  print (padrel(ra, w) == padrel(rb, w) && sa == sb) ? 1 : 0
}
'

# pep440_canon <version> — canonical spelling of <version> on stdout; non-zero
# when <version> carries no release segment to canonicalize.
pep440_canon() {
  awk -v mode=canon -v v="$1" "$PEP440_AWK" /dev/null
}

# version_eq <a> <b> — rc 0 iff <a> and <b> are the same PEP 440 version. Exact
# equality is the fast path; anything else is decided by the rule above, and a
# value the rule cannot canonicalize is only ever equal to itself.
version_eq() {
  [ "$1" = "$2" ] && return 0
  [ "$(awk -v mode=eq -v a="$1" -v b="$2" "$PEP440_AWK" /dev/null)" = "1" ]
}
