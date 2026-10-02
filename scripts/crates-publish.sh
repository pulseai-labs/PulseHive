#!/usr/bin/env bash
# crates-publish.sh — gated crates.io publish for one v* release (r2.s3.w2).
#
# The release gate packages the five workspace crates once and uploads those
# exact bytes as `tested-crates`; this script is the publish job's half of that
# contract. It never trusts the registry to be empty and it never trusts a
# version that is already there: a crate already on the index is skipped only
# when its cksum is the tested one, and refused — with every later crate left
# unpublished — when it is not.
#
# Modes:
#   package --out <dir>
#       One verified `cargo package` invocation with a `-p` per manifest crate
#       (`scripts/release-check.sh --get rust.crates`, in that order), then the
#       `.crate` files, the workspace `Cargo.lock` the packaging resolved, and a
#       `SHA256SUMS` land in <dir>. Prints `crates-publish: packaged <n> crates`.
#   verify-local --tested <dir>
#       The tested resolution is restored first (the Cargo.lock in <dir> is
#       copied to the workspace root — missing means unknown, refuse). The set
#       is then repackaged with `--no-verify --locked`, the same joint
#       invocation the gate used, and every `.crate` must equal SHA256SUMS; a
#       difference is a named refusal. Prints
#       `crates-publish: <n> crates match the tested packages`.
#       (`publish` repeats this joint repackage for the remaining set just
#       before upload — see do_publish.)
#   plan --tested <dir>
#       Read-only: read the index for each crate and print its decision. Exits
#       non-zero if any crate would be refused, so the pre-approval inventory
#       cannot read "publishable" over a refusal. Prints `crates-publish: plan ok`.
#   publish --tested <dir> [--fresh-build]
#       `verify-local`, then the decision table for every crate in manifest
#       order: a version already present with the tested cksum is skipped, one
#       present with different bytes (or yanked) refuses the run before anything
#       is uploaded. The remaining set — exactly the crates still to publish —
#       is repackaged jointly one last time and must reproduce the tested bytes,
#       and is then uploaded in ONE `cargo publish` invocation (multi `-p`,
#       `--no-verify --locked`): the joint form is what makes the uploaded
#       `.crate` files the tested ones, because cargo resolves an
#       interdependent multi-`-p` set the way the gate's packaging did. Each
#       uploaded crate's index cksum is polled afterwards (bounded, no fixed
#       sleeps) and required to equal the tested hash. Prints
#       `crates-publish: published <n>, skipped <m>`.
#       --fresh-build is how the workflow marks a `workflow_dispatch` run
#       (a fresh build); the different-bytes refusal then names the remedy:
#       re-run the failed jobs of the original tag run, which is the only path
#       that reuses the tested artifacts.
#   --self-test
#       Hermetic (no cargo, no network, no credentials): replaces the three
#       seams below — the index fetch, `cargo publish`, and the packaging — and
#       runs the shipped logic and nothing else. Prints `case <label>: <outcome>`
#       per case and `self-test: ok` last.
#
# Every failure is a named line on stderr (`crates-publish: ERROR: ...`) so the
# release runbook can map it to a recovery action (ADR-007). Refusals exit
# non-zero and publish nothing further; a missing token fails the run before any
# network call (A4). Nothing here logs in: the short-lived trusted-publishing
# token arrives in CARGO_REGISTRY_TOKEN from the approved job.
#
# The three seams — index_fetch, cargo_publish and package_crates — are the only
# places this script touches the network or cargo. `--self-test` replaces those
# three and nothing else, following py-publish-check.sh's pattern of stubbing
# only the HTTP layer.
set -u

PROG="crates-publish"
SCRIPTS_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
SELF="$SCRIPTS_DIR/$(basename -- "${BASH_SOURCE[0]}")"
CHECK="$SCRIPTS_DIR/release-check.sh"

# The tree the publish operates on: the workspace root whose Cargo.lock is the
# frozen resolution and whose target/package holds cargo's output. The
# self-test points it at a throwaway tree, so no run of --self-test writes a
# real workspace.
ROOT="$(cd -- "$SCRIPTS_DIR/.." && pwd)"

INDEX_URL="https://index.crates.io"
# The marker that separates a fetched body from its status code; the self-test's
# index stub writes the same shape, so only the transport is ever replaced.
INDEX_STATUS_MARKER="__crates_publish_status__"
# The post-publish poll: bounded, no fixed sleep before the first read.
INDEX_WAIT_ATTEMPTS=30
INDEX_WAIT_SECONDS=5

# Manifest values, read once in main().
CRATES=()
VERSION=""

# Directories this run must remove; SELFTEST_TMP is the self-test's fixture
# tree, SCRATCH a joint repackage's output (verify-local's or publish's).
SCRATCH=""
SELFTEST_TMP=""

# Parsed arguments: set by parse_args, read by dispatch (main() keeps to the
# house 80-line bound by handing both off).
ARG_TARGET=""
ARG_FRESH=0
ARG_SAW_OUT=0
ARG_SAW_TESTED=0

cleanup() {
  [ -z "${SCRATCH:-}" ] || rm -rf "$SCRATCH"
  [ -z "${SELFTEST_TMP:-}" ] || rm -rf "$SELFTEST_TMP"
}
trap cleanup EXIT

usage() {
  cat <<EOF
usage:
  $PROG package --out <dir>
  $PROG verify-local --tested <dir>
  $PROG plan --tested <dir>
  $PROG publish --tested <dir> [--fresh-build]
  $PROG --self-test
EOF
}

note() {
  printf '%s: %s\n' "$PROG" "$*"
}

error_line() {
  printf '%s: ERROR: %s\n' "$PROG" "$*" >&2
}

fail() {
  error_line "$*"
  exit 1
}

# read_manifest — the publish order and the version come from the manifest
# through w1's checker; this script keeps no second copy of either.
read_manifest() {
  local crates
  crates="$(bash "$CHECK" --get rust.crates)" ||
    fail "cannot read rust.crates from $CHECK"
  VERSION="$(bash "$CHECK" --get rust.version)" ||
    fail "cannot read rust.version from $CHECK"
  read -r -a CRATES <<<"$crates"
  [ "${#CRATES[@]}" -gt 0 ] || fail "the manifest names no rust crates"
}

# index_path <crate> — the crates.io sparse-index path: `1/`, `2/` or `3/<c1>/`
# for one-, two- and three-character names, `<c1c2>/<c3c4>/` otherwise
# (pulsehive-core -> pu/ls/pulsehive-core).
index_path() {
  local crate="$1"
  case "${#crate}" in
    1) printf '1/%s' "$crate" ;;
    2) printf '2/%s' "$crate" ;;
    3) printf '3/%s/%s' "${crate:0:1}" "$crate" ;;
    *) printf '%s/%s/%s' "${crate:0:2}" "${crate:2:2}" "$crate" ;;
  esac
}

# tested_hash <dir> <crate> — the sha256 `SHA256SUMS` records for
# <crate>-<version>.crate; empty when the file has no line for it.
tested_hash() {
  awk -v name="$2-$VERSION.crate" \
    '$2 == name || $2 == "./" name { print $1; exit }' "$1/SHA256SUMS"
}

# ---------------------------------------------------------------------------
# Seams
# ---------------------------------------------------------------------------

# index_fetch <crate> — SEAM. The HTTP read of the sparse index, printed as the
# body followed by INDEX_STATUS_MARKER<status>; rc 7 when the request itself
# failed (DNS, TLS, timeout). --self-test replaces this function.
index_fetch() {
  local url
  url="$INDEX_URL/$(index_path "$1")"
  curl -sS --max-time 30 -w "$INDEX_STATUS_MARKER"'%{http_code}' "$url" 2>/dev/null || return 7
}

# cargo_publish <crate>... — SEAM. ONE `cargo publish` invocation for the whole
# set, in manifest order, `--no-verify --locked`: the bytes were verified by the
# gate's packaging and are re-checked immediately before this call, so the build
# is not repeated here, and the index cksum check after the upload is the
# backstop. Uploading the set jointly is what makes the uploaded bytes the
# tested bytes: cargo packages a multi-`-p` selection the way the gate did
# (interdependent crates resolved against a local overlay of the packages just
# built), while `cargo publish -p <crate>` alone packages that crate by itself
# and writes the registry's cksum for its workspace dependencies into the inner
# Cargo.lock — measured on this workspace, that path uploads adf8dec5… for
# pulsehive-openai against the tested 7ae33a15… (report §7). Nothing here logs
# in: the short-lived trusted-publishing token arrives in CARGO_REGISTRY_TOKEN.
# --self-test replaces this function.
cargo_publish() {
  local crate args=()
  for crate in "$@"; do
    args+=(-p "$crate")
  done
  ( cd "$ROOT" && cargo publish "${args[@]}" --no-verify --locked )
}

# package_crates <verify|no-verify> <out-dir> <crate>... — SEAM. One `cargo
# package` invocation for the named crates (verification on unless no-verify),
# then each `.crate` it wrote is copied into <out-dir>. The workspace Cargo.lock
# is what every `--locked` in this script validates against and what the
# `tested-crates` artifact carries; a fresh checkout has none (it is
# gitignored), so it is resolved here when absent. --self-test replaces this
# function.
package_crates() {
  local mode="$1" out="$2" crate args=()
  shift 2
  if [ ! -f "$ROOT/Cargo.lock" ]; then
    ( cd "$ROOT" && cargo generate-lockfile ) || return 1
  fi
  for crate in "$@"; do
    args+=(-p "$crate")
  done
  if [ "$mode" = "no-verify" ]; then
    args+=(--no-verify)
  fi
  ( cd "$ROOT" && cargo package "${args[@]}" --locked ) || return 1
  mkdir -p "$out" || return 1
  for crate in "$@"; do
    cp "$ROOT/target/package/$crate-$VERSION.crate" "$out/" || return 1
  done
}

# ---------------------------------------------------------------------------
# The index read and the per-crate decision
# ---------------------------------------------------------------------------

# index_read <crate> — one tab-separated record:
#   absent<TAB>                       the version is not on the index (404)
#   present<TAB><cksum><TAB><yanked>  the version is there
#   error<TAB><detail>                any other status, a transport failure, or
#                                     a body that does not parse — fail closed
index_read() {
  local crate="$1" raw status body record
  if ! raw="$(index_fetch "$crate")"; then
    printf 'error\tthe request itself failed\n'
    return 0
  fi
  status="${raw##*"$INDEX_STATUS_MARKER"}"
  body="${raw%"$INDEX_STATUS_MARKER"*}"
  case "$status" in
    404) printf 'absent\t\t\n'; return 0 ;;
    200) : ;;
    *) printf 'error\tHTTP %s\n' "$status"; return 0 ;;
  esac
  record="$(printf '%s\n' "$body" | python3 -c '
import json
import sys

version = sys.argv[1]
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    entry = json.loads(line)
    if entry.get("vers") == version:
        print("present\t%s\t%s" % (entry.get("cksum", ""), str(bool(entry.get("yanked", False))).lower()))
        break
else:
    print("absent\t\t")
' "$VERSION" 2>/dev/null)" || {
    printf 'error\tthe index body did not parse\n'
    return 0
  }
  printf '%s\n' "$record"
}

# crate_decision <crate> <tested-hash> <fresh-build 0|1>
#   Prints the crate's decision line and returns:
#     0  publish   `crates-publish: <crate> <v>: publishing`
#     1  skip      `crates-publish: <crate> <v> already published with the
#                   tested bytes — skipped`
#     2  refused   the named refusal is already on stderr; the caller must stop
crate_decision() {
  local crate="$1" tested="$2" fresh="$3" record kind cksum yanked message
  record="$(index_read "$crate")"
  kind="${record%%$'\t'*}"
  case "$kind" in
    absent)
      note "$crate $VERSION: publishing"
      return 0
      ;;
    present)
      cksum="$(printf '%s' "$record" | cut -f2)"
      yanked="$(printf '%s' "$record" | cut -f3)"
      # A17: yanked is not complete, whatever the bytes say — the release does
      # not count as published until the operator decides (runbook).
      if [ "$yanked" = "true" ]; then
        error_line "$crate $VERSION is yanked on crates.io — not complete; unyanking is the operator's decision (docs/RELEASING.md)"
        return 2
      fi
      if [ "$cksum" = "$tested" ]; then
        note "$crate $VERSION already published with the tested bytes — skipped"
        return 1
      fi
      message="$crate $VERSION is already on crates.io with different bytes (index $cksum, tested $tested) — refusing"
      [ "$fresh" -eq 0 ] ||
        message="$message — this is a fresh build; re-run the failed jobs of the original tag run instead"
      error_line "$message"
      return 2
      ;;
    *)
      error_line "cannot read the crates.io index for $crate (${record#error$'\t'}) — refusing"
      return 2
      ;;
  esac
}

# ---------------------------------------------------------------------------
# The flows
# ---------------------------------------------------------------------------

# adopt_lock <dir> — the tested resolution travels inside the artifact: every
# cargo call after packaging runs `--locked` against the Cargo.lock `package`
# wrote into <dir>. An artifact without one cannot be republished identically,
# so it is refused rather than re-resolved.
adopt_lock() {
  local dir="$1"
  [ -f "$dir/Cargo.lock" ] ||
    fail "no Cargo.lock in $dir — the tested resolution is unknown, refusing"
  cp "$dir/Cargo.lock" "$ROOT/Cargo.lock" ||
    fail "cannot copy $dir/Cargo.lock to $ROOT/Cargo.lock"
}

# compare_crate <crate> <tested-dir> <rebuilt-crate-file> — the named
# byte-identity check: the tested hash against the repackaged `.crate`.
compare_crate() {
  local crate="$1" dir="$2" rebuilt_file="$3" tested rebuilt
  [ -f "$rebuilt_file" ] ||
    fail "the repackaged $crate-$VERSION.crate is missing from $rebuilt_file"
  tested="$(tested_hash "$dir" "$crate")"
  [ -n "$tested" ] ||
    fail "no sha256 for $crate-$VERSION.crate in $dir/SHA256SUMS"
  rebuilt="$(sha256sum "$rebuilt_file" | awk '{print $1}')"
  [ "$tested" = "$rebuilt" ] ||
    fail "$crate-$VERSION.crate differs from the tested package (tested $tested, rebuilt $rebuilt)"
}

do_package() {
  local out="$1" crate
  mkdir -p "$out" || fail "cannot create $out"
  package_crates verify "$out" "${CRATES[@]}" ||
    fail "cargo package failed for ${CRATES[*]}"
  [ -f "$ROOT/Cargo.lock" ] ||
    fail "no workspace Cargo.lock after packaging — the tested resolution is unknown, refusing"
  cp "$ROOT/Cargo.lock" "$out/Cargo.lock" ||
    fail "cannot copy the resolved Cargo.lock into $out"
  ( cd "$out" && sha256sum ./*.crate >SHA256SUMS ) ||
    fail "cannot write $out/SHA256SUMS"
  note "packaged ${#CRATES[@]} crates"
}

# do_verify_local <dir> — the tested artifacts are reproducible from this tree
# under the frozen lock: one `--no-verify --locked` repackage of the whole set
# must reproduce every `.crate` byte for byte.
#
# Deliberately the joint invocation, and never a set of single-crate ones:
# measured on 2026-10-02 against this workspace, `cargo package` writes a
# dependent crate's inner Cargo.lock with the *local overlay* checksum of the
# crate it just built, while packaging that crate alone writes the *registry*
# cksum of that dependency — so the two differ whenever an intra-workspace
# dependency is already published with other bytes (this tree's pulsehive-core
# 3.0.0: 9367f29b… against 7b102944…, the diff confined to that one Cargo.lock
# line), and the single-crate form cannot be produced at all while a dependency
# is unpublished (`cargo package -p <crate>`: "no matching package named …
# found"). Publishing follows the same rule, so the upload path is joint too —
# see cargo_publish and do_publish.
do_verify_local() {
  local dir="$1" crate
  adopt_lock "$dir"
  SCRATCH="$(mktemp -d "${TMPDIR:-/tmp}/crates-publish-verify.XXXXXX")" ||
    fail "cannot create a scratch dir for the repackage"
  package_crates no-verify "$SCRATCH" "${CRATES[@]}" ||
    fail "cargo package --no-verify failed for ${CRATES[*]}"
  for crate in "${CRATES[@]}"; do
    compare_crate "$crate" "$dir" "$SCRATCH/$crate-$VERSION.crate"
  done
  rm -rf "$SCRATCH"
  SCRATCH=""
  note "${#CRATES[@]} crates match the tested packages"
}

do_plan() {
  local dir="$1" crate tested rc refused=0
  adopt_lock "$dir"
  for crate in "${CRATES[@]}"; do
    tested="$(tested_hash "$dir" "$crate")"
    [ -n "$tested" ] ||
      fail "no sha256 for $crate-$VERSION.crate in $dir/SHA256SUMS"
    rc=0
    crate_decision "$crate" "$tested" 0 || rc=$?
    [ "$rc" -ne 2 ] || refused=1
  done
  [ "$refused" -eq 0 ] || exit 1
  note "plan ok"
}

# index_wait <crate> — poll the index until <crate> <version> appears and print
# its cksum; rc 1 when the bound expires or the index cannot be read.
index_wait() {
  local crate="$1" attempt=0 record
  while [ "$attempt" -lt "$INDEX_WAIT_ATTEMPTS" ]; do
    attempt=$((attempt + 1))
    record="$(index_read "$crate")"
    case "$record" in
      present$'\t'*)
        printf '%s\n' "$(printf '%s' "$record" | cut -f2)"
        return 0
        ;;
      absent$'\t'*)
        [ "$attempt" -lt "$INDEX_WAIT_ATTEMPTS" ] || break
        sleep "$INDEX_WAIT_SECONDS"
        ;;
      *) break ;;
    esac
  done
  return 1
}

do_publish() {
  local dir="$1" fresh="$2" crate tested rc cksum publish_set=() skipped=0
  [ -n "${CARGO_REGISTRY_TOKEN:-}" ] ||
    fail "no crates.io token (did the trusted-publishing exchange fail?) — refusing"
  do_verify_local "$dir"

  # Decide every crate first, in manifest order. The publish set is exactly the
  # crates the decision table marks `publish`; a crate already present with the
  # tested bytes is skipped, never re-uploaded, and a refusal stops the run
  # before anything at all is uploaded.
  for crate in "${CRATES[@]}"; do
    tested="$(tested_hash "$dir" "$crate")"
    [ -n "$tested" ] ||
      fail "no sha256 for $crate-$VERSION.crate in $dir/SHA256SUMS"
    rc=0
    crate_decision "$crate" "$tested" "$fresh" || rc=$?
    case "$rc" in
      0) publish_set+=("$crate") ;;
      1) skipped=$((skipped + 1)) ;;
      *) exit 1 ;;
    esac
  done

  if [ "${#publish_set[@]}" -gt 0 ]; then
    # The tested bytes or nothing: repackage exactly this set, jointly — the way
    # the upload packages it — and require every `.crate` to equal SHA256SUMS
    # before a single byte leaves the machine.
    SCRATCH="$(mktemp -d "${TMPDIR:-/tmp}/crates-publish-upload.XXXXXX")" ||
      fail "cannot create a scratch dir for the pre-upload repackage"
    package_crates no-verify "$SCRATCH" "${publish_set[@]}" ||
      fail "cargo package --no-verify failed for ${publish_set[*]}"
    for crate in "${publish_set[@]}"; do
      compare_crate "$crate" "$dir" "$SCRATCH/$crate-$VERSION.crate"
    done
    rm -rf "$SCRATCH"
    SCRATCH=""

    cargo_publish "${publish_set[@]}" ||
      fail "cargo publish failed for ${publish_set[*]}"

    # The index cksum backstop, per uploaded crate.
    for crate in "${publish_set[@]}"; do
      tested="$(tested_hash "$dir" "$crate")"
      cksum="$(index_wait "$crate")" ||
        fail "$crate $VERSION published, but the index never showed the version — refusing"
      [ "$cksum" = "$tested" ] ||
        fail "$crate $VERSION published, but the index cksum $cksum is not the tested $tested"
    done
  fi

  note "published ${#publish_set[@]}, skipped $skipped"
}

# ---------------------------------------------------------------------------
# --self-test
#
# Hermetic: the three seams are replaced, the shipped logic is not. Fixture
# values come from the shipped manifest, and every mutation is asserted to have
# applied, so a fixture that stops being a mutation fails the self-test instead
# of passing vacuously.
# ---------------------------------------------------------------------------

# SPINE.md L5's measured pair: the hash the pinned 1.98.1 packaging produced for
# pulsehive-core 3.0.0, and the cksum crates.io records for the 3.0.0 built by
# another toolchain. The pair is this work item's real negative control.
SELFTEST_REAL_TESTED_SHA="9bdf73630cf9d8047ff01a7ca5592caa9c1f6e7aa5a0334571f21696e0e20329"
SELFTEST_REAL_INDEX_SHA="7b1029446574756342e315c99226f583941e76d87ba7ce07b96ebc4ee2b44171"

SELFTEST_OUT=""
SELFTEST_RC=0

selftest_fail() { # <label> <detail>
  printf 'self-test: FAILED [%s]: %s\n' "$1" "$2" >&2
  exit 1
}

# selftest_call <function> <args...> — run through a command substitution, so a
# refusal's `exit 1` ends the case's subshell and not the self-test.
selftest_call() {
  SELFTEST_OUT="$("$@" 2>&1)"
  SELFTEST_RC=$?
}

# selftest_assert_refused <label> <named error> — the same assertion without a
# case line, for a flow that is checked a second time inside one case.
selftest_assert_refused() {
  [ "$SELFTEST_RC" -ne 0 ] ||
    selftest_fail "$1" "expected a rejection, got rc=0: $SELFTEST_OUT"
  case "$SELFTEST_OUT" in
    *"$2"*) ;;
    *) selftest_fail "$1" "rejected without the named error ('$2'): $SELFTEST_OUT" ;;
  esac
}

selftest_expect_rejected() { # <label> <named error>
  selftest_assert_refused "$1" "$2"
  printf 'case %s: rejected\n' "$1"
}

selftest_expect_accepted() { # <label> <success line>
  [ "$SELFTEST_RC" -eq 0 ] ||
    selftest_fail "$1" "expected acceptance, got rc=$SELFTEST_RC: $SELFTEST_OUT"
  case "$SELFTEST_OUT" in
    *"$2"*) ;;
    *) selftest_fail "$1" "accepted without the success line ('$2'): $SELFTEST_OUT" ;;
  esac
  printf 'case %s: %s\n' "$1" "${2#"$PROG: "}"
}

selftest_no_case() { # <label> <substring that must not appear>
  case "$SELFTEST_OUT" in
    *"$2"*) selftest_fail "$1" "the output must not carry '$2': $SELFTEST_OUT" ;;
  esac
}

# selftest_other_cksum / selftest_mismatched_cksum — deterministic hashes for
# the fixtures' "different bytes", distinct from any tested hash.
selftest_other_cksum() {
  printf 'not-the-tested-bytes-%s\n' "$1" | sha256sum | awk '{print $1}'
}

selftest_mismatched_cksum() {
  printf 'mismatched-after-publish-%s\n' "$1" | sha256sum | awk '{print $1}'
}

# selftest_index_entry <crate> <status> [<cksum> <yanked>] — one fake index
# entry: the status code on the first line, the JSON body after it (200 only).
selftest_index_entry() {
  local crate="$1" status="$2" cksum="${3:-}" yanked="${4:-false}"
  if [ "$status" = "200" ]; then
    printf '200\n{"name":"%s","vers":"%s","deps":[],"cksum":"%s","features":{},"yanked":%s,"links":null}\n' \
      "$crate" "$VERSION" "$cksum" "$yanked" >"$SELFTEST_INDEX_DIR/$crate"
  else
    printf '%s\n' "$status" >"$SELFTEST_INDEX_DIR/$crate"
  fi
}

# selftest_index_reset — a fresh fake index for the next case (a new directory
# rather than a delete-and-recreate, so a case can never read a predecessor's
# entries) and empty call logs.
selftest_index_reset() {
  SELFTEST_INDEX_GEN=$((SELFTEST_INDEX_GEN + 1))
  SELFTEST_INDEX_DIR="$SELFTEST_TMP/index-$SELFTEST_INDEX_GEN"
  mkdir -p "$SELFTEST_INDEX_DIR" || selftest_fail "fixture" "cannot create $SELFTEST_INDEX_DIR"
  : >"$SELFTEST_INDEX_LOG"
  : >"$SELFTEST_PUBLISH_LOG"
}

selftest_index_cksum() { # <crate> — the cksum in the fake index, empty when absent
  local file="$SELFTEST_INDEX_DIR/$1"
  [ -f "$file" ] || return 0
  python3 -c '
import json
import sys

lines = open(sys.argv[1], encoding="utf-8").read().splitlines()
if len(lines) < 2:
    raise SystemExit(0)
print(json.loads(lines[1])["cksum"])
' "$file" 2>/dev/null
}

# selftest_tested_dir <dir> — the directory `package` writes: one `.crate` per
# manifest crate with the packaging stub's bytes, SHA256SUMS, and the Cargo.lock.
selftest_tested_dir() {
  local dir="$1" crate
  mkdir -p "$dir" || selftest_fail "fixture" "cannot create $dir"
  : >"$dir/SHA256SUMS"
  for crate in "${CRATES[@]}"; do
    printf 'tested-%s\n' "$crate" >"$dir/$crate-$VERSION.crate"
    printf '%s  %s\n' \
      "$(sha256sum "$dir/$crate-$VERSION.crate" | awk '{print $1}')" \
      "$crate-$VERSION.crate" >>"$dir/SHA256SUMS"
  done
  printf 'lock\n' >"$dir/Cargo.lock"
}

# selftest_set_tested_hash <dir> <crate> <hash> — replace one SHA256SUMS row,
# refusing when the row is not there to replace.
selftest_set_tested_hash() {
  local dir="$1" crate="$2" hash="$3"
  awk -v name="$crate-$VERSION.crate" -v hash="$hash" '
    $2 == name { print hash "  " name; found = 1; next }
    { print }
    END { exit(found ? 0 : 1) }
  ' "$dir/SHA256SUMS" >"$dir/SHA256SUMS.new" ||
    selftest_fail "fixture" "no SHA256SUMS row for $crate-$VERSION.crate in $dir"
  mv "$dir/SHA256SUMS.new" "$dir/SHA256SUMS"
}

# selftest_repacked_hash <crate> — the sha256 the packaging stub produces for a
# crate it packages fresh.
selftest_repacked_hash() {
  printf 'tested-%s\n' "$1" | sha256sum | awk '{print $1}'
}

# selftest_published_identical <label> <dir> <crate>... — every named crate's
# fake index entry now carries exactly its tested cksum.
selftest_published_identical() {
  local label="$1" dir="$2" crate got want
  shift 2
  for crate in "$@"; do
    got="$(selftest_index_cksum "$crate")"
    want="$(tested_hash "$dir" "$crate")"
    [ -n "$got" ] && [ "$got" = "$want" ] ||
      selftest_fail "$label" "$crate: index cksum '$got' is not the tested '$want'"
  done
}

selftest_published_nothing() { # <label> — the publish seam was never called
  [ ! -s "$SELFTEST_PUBLISH_LOG" ] ||
    selftest_fail "$1" "cargo publish was called: $(cat "$SELFTEST_PUBLISH_LOG")"
}

# selftest_publish_log_is <label> <crates...> — the publish seam was called
# exactly once, with exactly these crates, in this order. One line per
# invocation is what makes the joint call observable: a per-crate loop leaves
# one line per crate here.
selftest_publish_log_is() {
  local label="$1" want
  shift
  want="$*"
  [ "$(cat "$SELFTEST_PUBLISH_LOG")" = "$want" ] ||
    selftest_fail "$label" \
      "the upload must be one invocation of '$want': $(cat "$SELFTEST_PUBLISH_LOG")"
}

self_test() {
  SELFTEST_TMP="$(mktemp -d "${TMPDIR:-/tmp}/crates-publish-selftest.XXXXXX")" ||
    fail "self-test: cannot create a temp dir"
  SELFTEST_INDEX_DIR="$SELFTEST_TMP/index"
  SELFTEST_INDEX_GEN=0
  SELFTEST_INDEX_LOG="$SELFTEST_TMP/index-reads.log"
  SELFTEST_PUBLISH_LOG="$SELFTEST_TMP/publishes.log"
  ROOT="$SELFTEST_TMP/root" # the lock copies land in the fixture tree, never a real workspace
  mkdir -p "$ROOT" || fail "self-test: cannot create the fixture root"
  # A throwaway token: the publish seam is stubbed, so nothing is uploaded; the
  # real run's token is the short-lived one the trusted-publishing exchange
  # returns. The no-token case empties this on purpose.
  CARGO_REGISTRY_TOKEN="self-test-token"

  # The seams. Only these three are replaced; the decision table, the flows,
  # the ordering and every comparison run for real.
  index_fetch() { # <crate> (self-test: the fake index under SELFTEST_INDEX_DIR)
    printf '%s\n' "$1" >>"$SELFTEST_INDEX_LOG"
    if [ "${SELFTEST_INDEX_TRANSPORT_FAIL:-0}" -eq 1 ]; then
      return 7
    fi
    local file="$SELFTEST_INDEX_DIR/$1" status
    if [ ! -f "$file" ]; then
      printf '%s404' "$INDEX_STATUS_MARKER"
      return 0
    fi
    status="$(sed -n 1p "$file")"
    sed -n '2,$p' "$file"
    printf '%s%s' "$INDEX_STATUS_MARKER" "$status"
    return 0
  }
  cargo_publish() { # <crate>... (self-test: ONE log line per invocation, then publish the set into the fake index)
    local crate cksum
    printf '%s\n' "$*" >>"$SELFTEST_PUBLISH_LOG"
    for crate in "$@"; do
      cksum="$(tested_hash "$SELFTEST_TESTED_DIR" "$crate")"
      if [ "${SELFTEST_POST_PUBLISH_MISMATCH:-}" = "$crate" ]; then
        cksum="$(selftest_mismatched_cksum "$crate")"
      fi
      selftest_index_entry "$crate" 200 "$cksum" false
    done
  }
  package_crates() { # <verify|no-verify> <out-dir> <crate>... (self-test: write the stub bytes)
    local mode="$1" out="$2" crate content calls_file calls
    shift 2
    mkdir -p "$out" || return 1
    for crate in "$@"; do
      content="tested-$crate"
      if [ -n "${SELFTEST_DRIFT_CRATE:-}" ] && [ "$crate" = "$SELFTEST_DRIFT_CRATE" ]; then
        calls_file="$SELFTEST_TMP/calls-$crate"
        calls="$(cat "$calls_file" 2>/dev/null || printf '0')"
        calls=$((calls + 1))
        printf '%s\n' "$calls" >"$calls_file"
        [ "$calls" -le 1 ] || content="drifted-$crate"
      fi
      printf '%s\n' "$content" >"$out/$crate-$VERSION.crate"
    done
    [ "$mode" = "verify" ] || return 0
    printf 'lock\n' >"$ROOT/Cargo.lock"
  }

  local n="${#CRATES[@]}" dir crate

  # 1. first-publish: nothing is on the index -> all five publish in manifest
  #    order, and every cksum the index then carries is the tested one.
  selftest_index_reset
  dir="$SELFTEST_TMP/first-publish"
  selftest_tested_dir "$dir"
  SELFTEST_TESTED_DIR="$dir"
  SELFTEST_POST_PUBLISH_MISMATCH=""
  selftest_call do_publish "$dir" 0
  selftest_expect_accepted "first-publish" "$PROG: published $n, skipped 0"
  selftest_publish_log_is "first-publish" "${CRATES[@]}"
  selftest_published_identical "first-publish" "$dir" "${CRATES[@]}"

  # 2. resume: the first two are already there with the tested bytes -> they are
  #    skipped and the remaining three publish, in order (L4's transient retry).
  selftest_index_reset
  dir="$SELFTEST_TMP/resume"
  selftest_tested_dir "$dir"
  SELFTEST_TESTED_DIR="$dir"
  for crate in "${CRATES[@]:0:2}"; do
    selftest_index_entry "$crate" 200 "$(tested_hash "$dir" "$crate")" false
  done
  selftest_call do_publish "$dir" 0
  selftest_expect_accepted "resume" "$PROG: published $((n - 2)), skipped 2"
  selftest_publish_log_is "resume" "${CRATES[@]:2}"
  selftest_published_identical "resume" "$dir" "${CRATES[@]}"

  # 3. complete: everything is present and identical -> nothing is published.
  selftest_index_reset
  dir="$SELFTEST_TMP/complete"
  selftest_tested_dir "$dir"
  SELFTEST_TESTED_DIR="$dir"
  for crate in "${CRATES[@]}"; do
    selftest_index_entry "$crate" 200 "$(tested_hash "$dir" "$crate")" false
  done
  selftest_call do_publish "$dir" 0
  selftest_expect_accepted "complete" "$PROG: published 0, skipped $n"
  selftest_published_nothing "complete"

  # 4. different-bytes: the third crate is present with another cksum -> refused,
  #    and the fourth and fifth are never published.
  selftest_index_reset
  dir="$SELFTEST_TMP/different-bytes"
  selftest_tested_dir "$dir"
  SELFTEST_TESTED_DIR="$dir"
  selftest_index_entry "${CRATES[2]}" 200 "$(selftest_other_cksum "${CRATES[2]}")" false
  selftest_call do_publish "$dir" 0
  local want_index want_tested
  want_index="$(selftest_other_cksum "${CRATES[2]}")"
  want_tested="$(tested_hash "$dir" "${CRATES[2]}")"
  selftest_expect_rejected "different-bytes" \
    "$PROG: ERROR: ${CRATES[2]} $VERSION is already on crates.io with different bytes (index $want_index, tested $want_tested) — refusing"
  selftest_no_case "different-bytes" "fresh build"
  selftest_published_nothing "different-bytes"

  # 5. index-error: an unreadable index refuses; a transport failure reads the
  #    same way (fail closed), and neither publishes anything.
  selftest_index_reset
  dir="$SELFTEST_TMP/index-error"
  selftest_tested_dir "$dir"
  SELFTEST_TESTED_DIR="$dir"
  selftest_index_entry "${CRATES[0]}" 500
  selftest_call do_publish "$dir" 0
  selftest_expect_rejected "index-error" \
    "$PROG: ERROR: cannot read the crates.io index for ${CRATES[0]} (HTTP 500) — refusing"
  SELFTEST_INDEX_TRANSPORT_FAIL=1
  selftest_call do_publish "$dir" 0
  SELFTEST_INDEX_TRANSPORT_FAIL=0
  selftest_assert_refused "index-error" \
    "$PROG: ERROR: cannot read the crates.io index for ${CRATES[0]} (the request itself failed) — refusing"
  selftest_published_nothing "index-error"

  # 6. post-publish-mismatch: the upload goes out, the index then carries other
  #    bytes -> refused with the named line, and nothing further is published.
  selftest_index_reset
  dir="$SELFTEST_TMP/post-publish-mismatch"
  selftest_tested_dir "$dir"
  SELFTEST_TESTED_DIR="$dir"
  SELFTEST_POST_PUBLISH_MISMATCH="${CRATES[0]}"
  selftest_call do_publish "$dir" 0
  SELFTEST_POST_PUBLISH_MISMATCH=""
  want_index="$(selftest_mismatched_cksum "${CRATES[0]}")"
  want_tested="$(tested_hash "$dir" "${CRATES[0]}")"
  selftest_expect_rejected "post-publish-mismatch" \
    "$PROG: ERROR: ${CRATES[0]} $VERSION published, but the index cksum $want_index is not the tested $want_tested"
  selftest_publish_log_is "post-publish-mismatch" "${CRATES[@]}"

  # 7. no-token: an empty CARGO_REGISTRY_TOKEN refuses before any network call
  #    and before the publish seam is reached (A4).
  selftest_index_reset
  dir="$SELFTEST_TMP/no-token"
  selftest_tested_dir "$dir"
  SELFTEST_TESTED_DIR="$dir"
  CARGO_REGISTRY_TOKEN= selftest_call do_publish "$dir" 0
  selftest_expect_rejected "no-token" \
    "$PROG: ERROR: no crates.io token (did the trusted-publishing exchange fail?) — refusing"
  [ ! -s "$SELFTEST_INDEX_LOG" ] ||
    selftest_fail "no-token" "the index was read before the token check: $(cat "$SELFTEST_INDEX_LOG")"
  selftest_published_nothing "no-token"

  # 8. local-drift: a tampered SHA256SUMS makes verify-local refuse the crate
  #    whose bytes no longer match.
  selftest_index_reset
  dir="$SELFTEST_TMP/local-drift"
  selftest_tested_dir "$dir"
  SELFTEST_TESTED_DIR="$dir"
  selftest_set_tested_hash "$dir" "${CRATES[0]}" "$(selftest_other_cksum "${CRATES[0]}")"
  selftest_call do_verify_local "$dir"
  want_index="$(selftest_other_cksum "${CRATES[0]}")"
  want_tested="$(selftest_repacked_hash "${CRATES[0]}")"
  selftest_expect_rejected "local-drift" \
    "$PROG: ERROR: ${CRATES[0]}-$VERSION.crate differs from the tested package (tested $want_index, rebuilt $want_tested)"

  # 9. real-3.0.0: SPINE.md L5's measured pair — the tested bytes the pinned
  #    toolchain produced against the cksum crates.io recorded for a 3.0.0 built
  #    by another toolchain — must refuse as different bytes.
  selftest_index_reset
  dir="$SELFTEST_TMP/real-3.0.0"
  selftest_tested_dir "$dir"
  SELFTEST_TESTED_DIR="$dir"
  selftest_set_tested_hash "$dir" pulsehive-core "$SELFTEST_REAL_TESTED_SHA"
  selftest_index_entry pulsehive-core 200 "$SELFTEST_REAL_INDEX_SHA" false
  selftest_call do_plan "$dir"
  selftest_expect_rejected "real-3.0.0" \
    "$PROG: ERROR: pulsehive-core 3.0.0 is already on crates.io with different bytes (index $SELFTEST_REAL_INDEX_SHA, tested $SELFTEST_REAL_TESTED_SHA) — refusing"

  # 10. yanked-identical: a yanked entry is not complete whatever its bytes say
  #     (A17) — a version matching the tested hash exactly is still refused.
  selftest_index_reset
  dir="$SELFTEST_TMP/yanked-identical"
  selftest_tested_dir "$dir"
  SELFTEST_TESTED_DIR="$dir"
  selftest_index_entry "${CRATES[0]}" 200 "$(tested_hash "$dir" "${CRATES[0]}")" true
  selftest_call do_publish "$dir" 0
  selftest_expect_rejected "yanked-identical" \
    "$PROG: ERROR: ${CRATES[0]} $VERSION is yanked on crates.io — not complete; unyanking is the operator's decision (docs/RELEASING.md)"
  selftest_published_nothing "yanked-identical"

  # 11. missing-lock: an artifact without the tested Cargo.lock is refused by
  #     every flow that would repackage or decide from it.
  selftest_index_reset
  dir="$SELFTEST_TMP/missing-lock"
  selftest_tested_dir "$dir"
  SELFTEST_TESTED_DIR="$dir"
  mv "$dir/Cargo.lock" "$SELFTEST_TMP/missing-lock.Cargo.lock"
  selftest_call do_verify_local "$dir"
  selftest_expect_rejected "missing-lock" \
    "$PROG: ERROR: no Cargo.lock in $dir — the tested resolution is unknown, refusing"
  selftest_call do_plan "$dir"
  selftest_assert_refused "missing-lock" \
    "$PROG: ERROR: no Cargo.lock in $dir — the tested resolution is unknown, refusing"
  selftest_call do_publish "$dir" 0
  selftest_assert_refused "missing-lock" \
    "$PROG: ERROR: no Cargo.lock in $dir — the tested resolution is unknown, refusing"
  selftest_published_nothing "missing-lock"

  # 12. pre-upload-mismatch: the joint pre-upload repackage differs from the
  #     tested set -> refused, and not one crate of the set is uploaded.
  selftest_index_reset
  dir="$SELFTEST_TMP/pre-upload-mismatch"
  selftest_tested_dir "$dir"
  SELFTEST_TESTED_DIR="$dir"
  SELFTEST_DRIFT_CRATE="${CRATES[2]}"
  selftest_call do_publish "$dir" 0
  SELFTEST_DRIFT_CRATE=""
  want_index="$(tested_hash "$dir" "${CRATES[2]}")"
  want_tested="$(printf 'drifted-%s\n' "${CRATES[2]}" | sha256sum | awk '{print $1}')"
  selftest_expect_rejected "pre-upload-mismatch" \
    "$PROG: ERROR: ${CRATES[2]}-$VERSION.crate differs from the tested package (tested $want_index, rebuilt $want_tested)"
  selftest_published_nothing "pre-upload-mismatch"

  # 13. fresh-build-hint: the same refusal on a fresh build (workflow_dispatch)
  #     names the remedy — re-run the original run, which is the path that
  #     reuses the tested artifacts (L4 narrowed).
  selftest_index_reset
  dir="$SELFTEST_TMP/fresh-build-hint"
  selftest_tested_dir "$dir"
  SELFTEST_TESTED_DIR="$dir"
  selftest_index_entry "${CRATES[2]}" 200 "$(selftest_other_cksum "${CRATES[2]}")" false
  selftest_call do_publish "$dir" 1
  want_index="$(selftest_other_cksum "${CRATES[2]}")"
  want_tested="$(tested_hash "$dir" "${CRATES[2]}")"
  selftest_expect_rejected "fresh-build-hint" \
    "$PROG: ERROR: ${CRATES[2]} $VERSION is already on crates.io with different bytes (index $want_index, tested $want_tested) — refusing — this is a fresh build; re-run the failed jobs of the original tag run instead"
  selftest_published_nothing "fresh-build-hint"

  # 14. publish-set-skips: a crate already present with the tested bytes is
  #     skipped, and the upload set is exactly the crates that still have to
  #     publish — one joint invocation, manifest order, skipped crates absent.
  selftest_index_reset
  dir="$SELFTEST_TMP/publish-set-skips"
  selftest_tested_dir "$dir"
  SELFTEST_TESTED_DIR="$dir"
  for crate in "${CRATES[0]}" "${CRATES[2]}"; do
    selftest_index_entry "$crate" 200 "$(tested_hash "$dir" "$crate")" false
  done
  selftest_call do_publish "$dir" 0
  selftest_expect_accepted "publish-set-skips" "$PROG: published $((n - 2)), skipped 2"
  selftest_publish_log_is "publish-set-skips" "${CRATES[1]}" "${CRATES[3]}" "${CRATES[4]}"
  selftest_published_identical "publish-set-skips" "$dir" "${CRATES[@]}"

  printf 'self-test: ok\n'
}

# parse_args [arg...] — the flag set, with the subcommand already taken off.
# Sets ARG_TARGET, ARG_FRESH, ARG_SAW_OUT and ARG_SAW_TESTED; exits 2 on a flag
# that is not understood or has no value.
parse_args() {
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --out)
        [ "$#" -ge 2 ] || {
          usage >&2
          exit 2
        }
        ARG_TARGET="$2"
        ARG_SAW_OUT=1
        shift 2
        ;;
      --tested)
        [ "$#" -ge 2 ] || {
          usage >&2
          exit 2
        }
        ARG_TARGET="$2"
        ARG_SAW_TESTED=1
        shift 2
        ;;
      --fresh-build)
        ARG_FRESH=1
        shift
        ;;
      -h | --help)
        usage
        exit 0
        ;;
      *)
        usage >&2
        printf '%s: unknown argument %s\n' "$PROG" "$1" >&2
        exit 2
        ;;
    esac
  done
}

# require_flag <saw 0|1> <subcommand> <flag> — a subcommand's own flag is not
# optional; exit 2 with the usage line that names it.
require_flag() {
  [ "$1" -eq 1 ] || {
    usage >&2
    printf '%s: %s needs %s\n' "$PROG" "$2" "$3" >&2
    exit 2
  }
}

# reject_fresh — --fresh-build belongs to publish alone.
reject_fresh() {
  [ "$ARG_FRESH" -eq 0 ] || {
    usage >&2
    printf '%s: --fresh-build is only for publish\n' "$PROG" >&2
    exit 2
  }
}

# dispatch <subcommand> — validate that subcommand's own flags, then run it.
dispatch() {
  case "$1" in
    package)
      require_flag "$ARG_SAW_OUT" package "--out <dir>"
      do_package "$ARG_TARGET"
      ;;
    verify-local)
      require_flag "$ARG_SAW_TESTED" verify-local "--tested <dir>"
      reject_fresh
      do_verify_local "$ARG_TARGET"
      ;;
    plan)
      require_flag "$ARG_SAW_TESTED" plan "--tested <dir>"
      reject_fresh
      do_plan "$ARG_TARGET"
      ;;
    publish)
      require_flag "$ARG_SAW_TESTED" publish "--tested <dir>"
      do_publish "$ARG_TARGET" "$ARG_FRESH"
      ;;
    *)
      usage >&2
      printf '%s: unknown subcommand %s\n' "$PROG" "$1" >&2
      exit 2
      ;;
  esac
}

main() {
  [ "$#" -gt 0 ] || {
    usage >&2
    exit 2
  }
  local sub="$1"
  shift
  parse_args "$@"
  read_manifest
  if [ "$sub" = "--self-test" ]; then
    self_test
    exit 0
  fi
  dispatch "$sub"
}

main "$@"
