#!/usr/bin/env bash
# py-publish-check.sh — fail-closed pre-publish gate for the `pulsehive` Python
# distribution (r2.s2.w2; r2.s3.w4 adds the identity chain, the published-state
# classification and the post-upload proof). RELEASE.md's negative control for
# the release path.
#
# Modes:
#   --dist <dir> --expect-name <name> --expect-version <version> [--fresh-build]
#       Verify that <dir> holds a complete, correctly-labelled, correctly-
#       versioned wheel set covering every advertised target; that every wheel
#       belongs to <name> — on its filename (#107) and in its own
#       *.dist-info/METADATA (V2); and that the version <version> names is in a
#       state this set may publish into. Prints `publish-mode: <mode>`, the
#       sha256 inventory, and — when $GITHUB_OUTPUT is set — appends
#       `publish-mode=<mode>` there. Every refusal exits non-zero with a named
#       error on stderr, after naming the mode it refused to publish in.
#
#       The modes, decided from PyPI's per-version JSON:
#
#         404                       full      nothing published
#         200, every published      complete  every published file is a tested
#             file byte-identical               wheel with the same sha256, and
#                                               every tested wheel is published
#         200, some tested wheels   resume    ...and some tested wheels are
#             missing                           missing; they are listed
#         200, a published file     refuse    it differs from the tested wheel,
#             with the same name                or it is not in the tested set
#             but other bytes, or
#             not in the tested set
#         200, a published file     refuse    even when its bytes match (A17) —
#             marked yanked                     unyanking is the operator's call
#         any other status, transport failure, unparseable JSON
#                                   refuse    fail closed, as before
#
#       With --fresh-build (the workflow passes it on workflow_dispatch, where
#       the run built new wheels) the differs-from-tested refusal carries the L4
#       hint: the recovery is to re-run the failed jobs of the original tag run,
#       never to publish the new bytes over the published ones.
#
#       <version> arrives as the `v*` tag's spelling of the version the manifest
#       declares, while the wheel filenames carry maturin's PEP 440 spelling of
#       it; the two are compared as versions through lib/pep440.sh, at every
#       site (the filename, the METADATA and the PyPI endpoint), so a prerelease
#       tag such as `v3.0.0-beta.1` matches the `3.0.0b1` wheels it names
#       instead of being refused for its spelling. Distribution names are
#       compared in PEP 503's normalized form. The chain tag -> manifest ->
#       filename -> METADATA -> installed version therefore has no unchecked
#       link before anything uploads.
#   --dist <dir> --expect-name <name> --expect-version <version> --verify-published
#       The post-upload half: every tested wheel must be on PyPI with the same
#       sha256. Retries a bounded number of times for PyPI's index to catch up
#       with the upload, then prints `verify-published: ok <n> file(s)` or fails
#       naming the file it could not prove.
#   --probe <project> <version>
#       Read-only helper: prints `<project> <version>: not published`, or
#       `<project> <version>: published <n> file(s)`.
#   --self-test
#       Hermetic (no network, no credentials): stubs only the HTTP layer (the
#       per-version JSON body as well as the status), builds fixture wheels as
#       throwaway zips under a temp dir, and asserts every rejection and every
#       accepted mode above — printing `case <label>: <outcome>` per case and
#       `self-test: ok` as its last line on success.
#
# The TARGETS list below is the single source of the advertised targets (L3);
# nothing else in this script or the release workflow keeps a second copy.

set -u

PROG="py-publish-check"
PYPI_JSON_URL="https://pypi.org/pypi"

# Advertised wheel targets (spine r2.s2, decision L3)
TARGETS=(
  "aarch64-apple-darwin"
  "x86_64-unknown-linux-gnu"
  "x86_64-pc-windows-msvc"
)

# The post-upload verification waits for PyPI's index to catch up: a bounded
# number of reads of the per-version JSON, then a named failure. The self-test
# drives the same loop with a delay of 0.
VERIFY_ATTEMPTS=6
VERIFY_DELAY_SECONDS=5

# Set by --fresh-build: a run the operator dispatched by hand built fresh wheels,
# so a published file that differs from the tested set is never superseded by
# the new bytes (L4 narrowed / A15).
FRESH_BUILD=0

# The run's scratch dir (mktemp -d, removed by the EXIT trap): holds the JSON
# body a read is judged from. The self-test points it at its own temp dir.
WORK_DIR=""

# Set by classify_published for its caller: the mode the run will publish in,
# and — for `resume` only — the tested wheels PyPI does not hold yet.
PUBLISH_MODE=""
PUBLISH_MISSING=""

# Rust target -> wheel platform-tag pattern (5th field of the wheel filename)
target_platform_pattern() {
  case "$1" in
    aarch64-apple-darwin)     printf '%s' 'macosx_.*arm64' ;;
    x86_64-unknown-linux-gnu) printf '%s' '(manylinux.*x86_64|linux_x86_64)' ;;
    x86_64-pc-windows-msvc)   printf '%s' 'win_amd64' ;;
    *) return 1 ;;
  esac
}

usage() {
  cat <<EOF
usage:
  $PROG --dist <dir> --expect-name <name> --expect-version <version> [--fresh-build]
  $PROG --dist <dir> --expect-name <name> --expect-version <version> --verify-published
  $PROG --probe <project> <version>
  $PROG --self-test
EOF
}

fail() {
  echo "$PROG: ERROR: $*" >&2
  exit 1
}

# pypi_version_url <project> <version>
#   The per-version JSON endpoint for <version>, in PEP 440's canonical
#   spelling — the spelling PyPI indexes, so the tag `v3.0.0-beta.1` asks about
#   `3.0.0b1` — and with the distribution name in PEP 503's normalized form,
#   which is the form PyPI's endpoint answers directly (a non-normalized
#   spelling gets a redirect this script would then have to fail closed on).
#   Pure and network-free: --self-test judges the URL a probe would use, and rc
#   non-zero means the version has no release segment to spell.
pypi_version_url() {
  local canon name
  canon=$(pep440_canon "$2") || return 1
  name=$(normalize_name "$1")
  printf '%s/%s/%s/json' "$PYPI_JSON_URL" "$name" "$canon"
}

# normalize_name <name> — PEP 503's normalized spelling: lowercased, with every
# run of `-`, `_` or `.` collapsed to one `-`. Both distribution-name
# comparisons (the filename's name field, #107, and the METADATA `Name`, V2)
# run on this form, so `PulseHive_SDK` and `pulsehive-sdk` are one
# distribution — which is how pip and PyPI read them.
normalize_name() {
  printf '%s' "$1" | tr '[:upper:]' '[:lower:]' | tr -s '._-' '-'
}

# read_wheel_metadata <wheel> — print `Name<TAB>Version` read from the wheel's
# own *.dist-info/METADATA (V2: the artifact's claim, never the filename's).
# rc non-zero when the wheel is not a readable zip or carries no METADATA; a
# missing field prints empty and the caller names the mismatch either way.
read_wheel_metadata() {
  python3 - "$1" <<'READ_METADATA'
import sys
import zipfile

try:
    with zipfile.ZipFile(sys.argv[1]) as archive:
        names = [n for n in archive.namelist() if n.endswith(".dist-info/METADATA")]
        if not names:
            sys.exit(3)
        text = archive.read(names[0]).decode("utf-8", "replace")
except (OSError, zipfile.BadZipFile):
    sys.exit(3)

name = version = ""
for line in text.splitlines():
    if not line.strip():
        break  # end of the RFC 822 headers
    key, sep, value = line.partition(":")
    if not sep:
        continue
    key = key.strip().lower()
    if key == "name" and not name:
        name = value.strip()
    elif key == "version" and not version:
        version = value.strip()
print(f"{name}\t{version}")
READ_METADATA
}

# wheel_sha256 <wheel> — the wheel's lowercase hex sha256: the value PyPI's JSON
# publishes in urls[].digests.sha256, and the one this gate compares against.
wheel_sha256() {
  python3 - "$1" <<'SHA256'
import hashlib
import sys

digest = hashlib.sha256()
with open(sys.argv[1], "rb") as handle:
    for chunk in iter(lambda: handle.read(1 << 16), b""):
        digest.update(chunk)
print(digest.hexdigest())
SHA256
}

# json_published_files <body-file> — one line per published file of a
# per-version JSON body: `<filename><TAB><sha256><TAB><yanked 0|1>`, in the
# body's order. rc non-zero when the body is not readable JSON of the expected
# shape: a state that cannot be read is a state that cannot be proved, and every
# caller fails closed on it.
json_published_files() {
  python3 - "$1" <<'PUBLISHED_FILES'
import json
import sys

try:
    with open(sys.argv[1], encoding="utf-8") as handle:
        data = json.load(handle)
except (OSError, json.JSONDecodeError):
    sys.exit(3)

urls = data.get("urls")
if not isinstance(urls, list):
    sys.exit(3)
for entry in urls:
    digests = entry.get("digests") if isinstance(entry, dict) else None
    filename = entry.get("filename") if isinstance(entry, dict) else None
    sha = digests.get("sha256") if isinstance(digests, dict) else None
    if not isinstance(filename, str) or not filename or not isinstance(sha, str) or not sha:
        sys.exit(3)
    print(f"{filename}\t{sha.lower()}\t{1 if entry.get('yanked') else 0}")
PUBLISHED_FILES
}

# pypi_http_get <url> <body-file>
#   The transport: GET <url>, write the body to <body-file> and print the HTTP
#   status code. Deliberately without curl -f: here the status IS the answer, so
#   a 404 has to stay readable instead of being flattened into a transport
#   error. rc non-zero when the request itself failed (DNS, TLS, timeout).
pypi_http_get() {
  local code
  code=$(curl -s -o "$2" -w '%{http_code}' --max-time 30 "$1" 2>/dev/null) || return 7
  printf '%s' "$code"
}

# pypi_version_get <project> <version> <body-file>
#   rc 0 — <version> is published for <project> (HTTP 200); <body-file> holds the
#          per-version JSON body
#   rc 1 — not published (HTTP 404; an absent project answers the same 404,
#          which is why a first-ever publish is allowed through)
#   rc 2 — the state could not be established: any other status, an unspellable
#          version, or a transport failure. Callers fail closed.
pypi_version_get() {
  local project="$1" version="$2" body="$3" url code
  url=$(pypi_version_url "$project" "$version") || return 2
  code=$(pypi_http_get "$url" "$body") || return 2
  case "$code" in
    200) return 0 ;;
    404) return 1 ;;
    *) return 2 ;;
  esac
}

# published_violation <published-lines> <fresh-build 0|1> <tested-name:sha>...
#   The first reason the published set cannot be published into, or nothing when
#   every published file is a tested wheel with the same sha256. The order is
#   deliberate: a file that is not in the tested set is reported as foreign
#   before anything else, a yanked file refuses even when its bytes match (A17),
#   and only then does a differing sha256 mean the L4 fresh-build mistake.
published_violation() {
  local published="$1" fresh="$2"
  shift 2
  local filename sha yanked entry base tested_sha
  while IFS=$'\t' read -r filename sha yanked; do
    [ -n "$filename" ] || continue
    tested_sha=""
    for entry in "$@"; do
      base="${entry%%:*}"
      if [ "$base" = "$filename" ]; then
        tested_sha="${entry#*:}"
        break
      fi
    done
    if [ -z "$tested_sha" ]; then
      printf "published file '%s' is not in the tested set — refusing" "$filename"
      return 0
    fi
    if [ "$yanked" = "1" ]; then
      printf "published file '%s' is yanked on PyPI — not complete; unyanking is the operator's decision (docs/RELEASING.md)" "$filename"
      return 0
    fi
    if [ "$tested_sha" != "$sha" ]; then
      if [ "$fresh" = "1" ]; then
        printf "published file '%s' differs from the tested wheel — refusing — this is a fresh build; re-run the failed jobs of the original tag run instead" "$filename"
      else
        printf "published file '%s' differs from the tested wheel — refusing" "$filename"
      fi
      return 0
    fi
  done <<<"$published"
  return 0
}

# published_missing <published-lines> <tested-name>... — the tested wheels the
# published body does not list, in the caller's order, space-separated.
published_missing() {
  local published="$1"
  shift
  local base name sha_field rest found out=""
  for base in "$@"; do
    found=0
    while IFS=$'\t' read -r name sha_field rest; do
      [ -n "$name" ] || continue
      if [ "$name" = "$base" ]; then
        found=1
        break
      fi
    done <<<"$published"
    [ "$found" -eq 1 ] || out="$out$base "
  done
  printf '%s' "${out% }"
}

# published_not_yet <published-lines> <tested-name:sha>... — the first tested
# wheel PyPI does not yet hold byte-identically, or nothing when it holds them
# all. The post-upload mirror of published_violation: after an upload only "not
# there yet" counts, because the bytes were already accepted.
published_not_yet() {
  local published="$1"
  shift
  local entry base sha name pub_sha rest found
  for entry in "$@"; do
    base="${entry%%:*}"
    sha="${entry#*:}"
    found=""
    while IFS=$'\t' read -r name pub_sha rest; do
      [ -n "$name" ] || continue
      if [ "$name" = "$base" ]; then
        found="$pub_sha"
        break
      fi
    done <<<"$published"
    if [ -z "$found" ]; then
      printf "wheel '%s' is not on PyPI" "$base"
      return 0
    fi
    if [ "$found" != "$sha" ]; then
      printf "wheel '%s' is on PyPI with a different sha256 (tested %s, PyPI %s)" "$base" "$sha" "$found"
      return 0
    fi
  done
  return 0
}

# emit_publish_mode <mode>
#   The mode is printed for every classification and, when $GITHUB_OUTPUT is
#   set, appended there as this step's `publish-mode` output. A15 conditions the
#   upload on that output, so it is emitted even when the mode is `refuse` and
#   the script is about to fail.
emit_publish_mode() {
  echo "publish-mode: $1"
  if [ -n "${GITHUB_OUTPUT:-}" ]; then
    printf 'publish-mode=%s\n' "$1" >>"$GITHUB_OUTPUT"
  fi
}

# collect_dist <dist-dir>
#   Prints the accepted wheel paths, one per line, and nothing else. The release
#   publishes wheels only, and the publish action uploads whatever is in dist/ —
#   with the sdist build gone, nothing else rejects a stray entry (a leftover
#   tarball, a zip, an editor backup, a build log, a directory a tool left
#   behind), which would otherwise be uploaded as an artifact no check ever
#   looked at. The test is on the entry itself, not only on its name: a directory
#   called `pulsehive-3.0.0-cp311-abi3-win_amd64.whl` is a directory.
collect_dist() { # <dist-dir>
  local dist_dir="$1" wheels=() others=() entry entry_name
  shopt -s nullglob dotglob
  wheels=("$dist_dir"/*.whl)
  for entry in "$dist_dir"/*; do
    entry_name="${entry##*/}"
    if [ -f "$entry" ] && [ ! -L "$entry" ] && [ "${entry_name%.whl}" != "$entry_name" ]; then
      continue # a regular *.whl file: the only thing dist/ may hold
    fi
    if [ -d "$entry" ]; then
      others+=("$entry_name/") # a directory (or a link to one)
    else
      others+=("$entry_name")
    fi
  done
  shopt -u nullglob dotglob
  [ "${#wheels[@]}" -gt 0 ] || fail "no wheels (*.whl) found in '$dist_dir'"
  [ "${#others[@]}" -eq 0 ] ||
    fail "non-wheel artifact(s) in '$dist_dir': ${others[*]} — the release publishes wheels (*.whl) only"
  printf '%s\n' "${wheels[@]}"
}

# check_wheel <wheel> <expect-version> <expect-name>
#   One wheel's whole identity, and the only place it is judged: the filename
#   fields (the #107 distribution name, the version, the platform), the
#   advertised-target match (A6/V4), and the wheel's own *.dist-info/METADATA
#   (V2 — the artifact's claim, never the filename's). Prints the advertised
#   target the wheel covers, so the caller can prove every target is covered;
#   any rejection fails the run through `fail`.
check_wheel() { # <wheel> <expect-version> <expect-name>
  local wheel="$1" expect_version="$2" expect_name="$3"
  local base="${wheel##*/}"
  base="${base%.whl}"
  local fields=()
  IFS='-' read -r -a fields <<< "$base"
  if [ "${#fields[@]}" -ne 5 ]; then
    fail "mislabelled wheel '$base': filename is not <name>-<version>-<python>-<abi>-<platform>"
  fi
  local version="${fields[1]}" platform="${fields[4]}"
  local target pattern matched_target="" meta meta_name meta_version

  # #107: the filename's distribution name must be the one --expect-name
  # declares. Compared in PEP 503's normalized form, because that is the form
  # PyPI keys a project on and the form pip resolves a wheel by.
  if [ "$(normalize_name "${fields[0]}")" != "$(normalize_name "$expect_name")" ]; then
    fail "wheel '$base' belongs to distribution '${fields[0]}', not '$expect_name'"
  fi

  for target in "${TARGETS[@]}"; do
    pattern=$(target_platform_pattern "$target") ||
      fail "internal: no platform pattern for target '$target'"
    if printf '%s' "$platform" | grep -qE "^(${pattern})\$"; then
      matched_target="$target"
      break
    fi
  done
  [ -n "$matched_target" ] ||
    fail "wheel '$base' is built for an excluded host ('$platform'); advertised targets: ${TARGETS[*]}"

  version_eq "$version" "$expect_version" ||
    fail "version disagreement: wheel '$base' carries version '$version' but --expect-version is '$expect_version' (compared as PEP 440 versions)"

  # V2: the wheel's own METADATA must agree with the same two values the
  # filename just agreed with, so the chain tag -> manifest -> filename ->
  # METADATA -> installed version has no unchecked link. The metadata version
  # is maturin's PEP 440 spelling, hence the same version rule.
  meta=$(read_wheel_metadata "$wheel") ||
    fail "wheel '$base' has no readable *.dist-info/METADATA — nothing proves which distribution and version it carries"
  IFS=$'\t' read -r meta_name meta_version <<< "$meta"
  [ "$(normalize_name "$meta_name")" = "$(normalize_name "$expect_name")" ] ||
    fail "wheel '$base' METADATA says Name '$meta_name', expected '$expect_name'"
  version_eq "$meta_version" "$expect_version" ||
    fail "wheel '$base' METADATA says Version '$meta_version', expected '$expect_version'"

  printf '%s' "$matched_target"
}

# classify_published <expect-name> <expect-version> <tested-name:sha>...
#   The published state, decided from PyPI's per-version JSON: full (nothing
#   published), complete (all of it is these wheels, byte for byte), resume
#   (some of these wheels are still missing), or refuse — the state that must
#   not be published into. Sets PUBLISH_MODE and PUBLISH_MISSING for the caller
#   (the latter only for `resume`); a refusal emits its mode and fails the run
#   here, because A15 conditions the upload on that output and the operator
#   reads the mode in the run summary next to the error.
classify_published() { # <expect-name> <expect-version> <tested-name:sha>...
  local expect_name="$1" expect_version="$2"
  shift 2
  local body="$WORK_DIR/body.json" rc=0 published reason=""
  PUBLISH_MODE=""
  PUBLISH_MISSING=""
  pypi_version_get "$expect_name" "$expect_version" "$body" || rc=$?
  case $rc in
    1) PUBLISH_MODE=full ;;
    0)
      published=$(json_published_files "$body") || {
        emit_publish_mode refuse
        fail "cannot read the per-version JSON for '$expect_name' '$expect_version' — refusing to publish over a state that cannot be proved"
      }
      reason=$(published_violation "$published" "$FRESH_BUILD" "$@")
      if [ -n "$reason" ]; then
        emit_publish_mode refuse
        fail "$reason"
      fi
      local names=() pair
      for pair in "$@"; do
        names+=("${pair%%:*}")
      done
      PUBLISH_MISSING=$(published_missing "$published" "${names[@]}")
      if [ -z "$PUBLISH_MISSING" ]; then
        PUBLISH_MODE=complete
      else
        PUBLISH_MODE=resume
      fi
      ;;
    *)
      emit_publish_mode refuse
      fail "cannot verify PyPI state for '$expect_name' '$expect_version' (the per-version JSON probe answered neither 200 nor 404) — refusing to publish an unproven version"
      ;;
  esac
}

check_dist() { # <dist-dir> <expect-version> <expect-name>
  local dist_dir="$1" expect_version="$2" expect_name="$3"
  [ -d "$dist_dir" ] || fail "dist directory '$dist_dir' not found"

  local listed
  listed=$(collect_dist "$dist_dir") || exit 1
  local wheels=() wheel
  while IFS= read -r wheel; do
    [ -n "$wheel" ] || continue
    wheels+=("$wheel")
  done <<< "$listed"

  local seen=" " target
  for wheel in "${wheels[@]}"; do
    target=$(check_wheel "$wheel" "$expect_version" "$expect_name") || exit 1
    seen="$seen$target "
  done

  for target in "${TARGETS[@]}"; do
    case "$seen" in
      *" $target "*) : ;;
      *) fail "missing wheel for advertised target '$target' (looked in '$dist_dir')" ;;
    esac
  done

  # The tested set as `name:sha256` pairs — the bytes every later comparison is
  # against. Hashed only now, once the set is known to be one this gate would
  # publish at all.
  local tested=() sha pair
  for wheel in "${wheels[@]}"; do
    sha=$(wheel_sha256 "$wheel") || fail "cannot hash wheel '${wheel##*/}'"
    tested+=("${wheel##*/}:$sha")
  done

  local mode missing=""
  classify_published "$expect_name" "$expect_version" "${tested[@]}"
  mode="$PUBLISH_MODE"
  missing="$PUBLISH_MISSING"

  emit_publish_mode "$mode"
  # A16: the pre-approval inventory — every tested wheel and the sha256 the
  # post-upload verification will compare against — so the run summary shows the
  # exact set the gate judged, not a count of it.
  for pair in "${tested[@]}"; do
    printf 'wheel-sha256: %s %s\n' "${pair%%:*}" "${pair#*:}"
  done
  if [ "$mode" = "resume" ]; then
    echo "$PROG: resume: missing wheel(s): $missing"
  fi
  echo "$PROG: ok: ${#wheels[@]} wheel(s) cover all ${#TARGETS[@]} advertised targets at version '$expect_version' (project '$expect_name') — publish-mode $mode"
}

# verify_published_files <dist-dir> <expect-version> <expect-name>
#   The post-upload half: every tested wheel must be on PyPI with the same
#   sha256, read back from PyPI's own JSON. A just-finished upload can take a
#   moment to appear, so this retries a bounded number of times and then fails
#   naming the file it could not prove.
verify_published_files() { # <dist-dir> <expect-version> <expect-name>
  local dist_dir="$1" expect_version="$2" expect_name="$3"
  [ -d "$dist_dir" ] || fail "dist directory '$dist_dir' not found"
  local wheels=() wheel
  shopt -s nullglob
  wheels=("$dist_dir"/*.whl)
  shopt -u nullglob
  [ "${#wheels[@]}" -gt 0 ] || fail "no wheels (*.whl) found in '$dist_dir'"

  local tested=() sha
  for wheel in "${wheels[@]}"; do
    sha=$(wheel_sha256 "$wheel") || fail "cannot hash wheel '${wheel##*/}'"
    tested+=("${wheel##*/}:$sha")
  done

  local body="$WORK_DIR/body.json" attempt fault="" rc=0 published
  for ((attempt = 1; attempt <= VERIFY_ATTEMPTS; attempt++)); do
    fault=""
    rc=0
    pypi_version_get "$expect_name" "$expect_version" "$body" || rc=$?
    case $rc in
      0)
        published=$(json_published_files "$body") ||
          fault="the per-version JSON for '$expect_name' '$expect_version' could not be read"
        if [ -z "$fault" ]; then
          fault=$(published_not_yet "$published" "${tested[@]}")
        fi
        ;;
      1) fault="'$expect_name' '$expect_version' is not published yet" ;;
      *) fault="the published state of '$expect_name' '$expect_version' could not be established" ;;
    esac
    if [ -z "$fault" ]; then
      echo "verify-published: ok ${#wheels[@]} file(s)"
      return 0
    fi
    [ "$attempt" -lt "$VERIFY_ATTEMPTS" ] && sleep "$VERIFY_DELAY_SECONDS"
  done
  fail "verify-published: $fault (after $VERIFY_ATTEMPTS attempt(s))"
}

# probe <project> <version> — read-only: report PyPI's state for one version and
# judge nothing. Anything but a readable 200 or a 404 fails closed, exactly as
# the gate itself would.
probe() { # <project> <version>
  local project="$1" version="$2" rc=0 published n
  pypi_version_get "$project" "$version" "$WORK_DIR/body.json" || rc=$?
  case $rc in
    1) echo "$project $version: not published" ;;
    0)
      published=$(json_published_files "$WORK_DIR/body.json") ||
        fail "cannot read the per-version JSON for '$project' '$version' — refusing to guess the published state"
      n=0
      if [ -n "$published" ]; then
        n=$(printf '%s\n' "$published" | wc -l)
        n=${n//[!0-9]/}
      fi
      echo "$project $version: published $n file(s)"
      ;;
    *) fail "cannot verify PyPI state for '$project' '$version' (the per-version JSON probe answered neither 200 nor 404)" ;;
  esac
}

self_test() {
  # tmp is deliberately global so the EXIT trap below can still see it
  tmp=""
  tmp=$(mktemp -d "${TMPDIR:-/tmp}/py-publish-check-selftest.XXXXXX") ||
    fail "self-test: cannot create a temp dir"
  trap 'rm -rf "${tmp:-}"' EXIT
  # The classification and verification paths read their JSON body from
  # $WORK_DIR; here that is the case fixtures' own temp dir. The verification
  # retry loop is the same loop the release path runs, with the propagation
  # wait taken out so the suite stays hermetic and quick.
  WORK_DIR="$tmp"
  VERIFY_DELAY_SECONDS=0

  # Hermetic: stub ONLY the HTTP layer (the status code and the JSON body). The
  # endpoint a read asks for, the rc mapping that turns a status code into
  # published / not published / cannot tell, and every judgement made from the
  # body all run for real, so a wrong endpoint, a wrong reading of a status or a
  # wrong reading of the body fails here offline instead of on the release path,
  # where the answers are live. The stub records every URL it is handed, can fail
  # like a dead network, and can call a case's own hook to serve a sequence (the
  # propagation case below).
  SELFTEST_HTTP_LOG="$tmp/http-requests.log"
  : > "$SELFTEST_HTTP_LOG"
  SELFTEST_HTTP_CODE="200"
  SELFTEST_HTTP_BODY="" # a file whose bytes are the body; empty means no body
  SELFTEST_HTTP_FAIL=0
  # The call number is read back off the request log, not kept in a shell
  # variable: every read runs inside a command substitution, so a variable
  # would reset to its parent's value on each call and a case that serves a
  # sequence could never see its second read.
  SELFTEST_HTTP_CALLS=0
  SELFTEST_HTTP_HOOK=""
  pypi_http_get() { # <url> <body-file>
    printf '%s\n' "$1" >> "$SELFTEST_HTTP_LOG"
    SELFTEST_HTTP_CALLS=$(grep -c '' "$SELFTEST_HTTP_LOG")
    if [ -n "${SELFTEST_HTTP_HOOK:-}" ]; then "$SELFTEST_HTTP_HOOK"; fi
    if [ "${SELFTEST_HTTP_FAIL:-0}" -eq 1 ]; then
      return 7
    fi
    if [ -n "${SELFTEST_HTTP_BODY:-}" ]; then
      cp "$SELFTEST_HTTP_BODY" "$2"
    else
      : > "$2"
    fi
    printf '%s' "$SELFTEST_HTTP_CODE"
  }
  expect_status() { # <label> <project> <version> <expected rc>
    local rc=0
    pypi_version_get "$2" "$3" "$tmp/status-body.json" || rc=$?
    [ "$rc" -eq "$4" ] || {
      echo "self-test: FAILED [$1]: pypi_version_get '$2' '$3' -> rc=$rc, expected rc=$4" >&2
      exit 1
    }
  }
  expect_url() { # <label> <expected url> <project> <version>
    local got
    got=$(pypi_version_url "$3" "$4") || {
      echo "self-test: FAILED [$1]: no URL for '$3' '$4'" >&2
      exit 1
    }
    [ "$got" = "$2" ] || {
      echo "self-test: FAILED [$1]: URL '$got', expected '$2'" >&2
      exit 1
    }
  }

  local ver="3.0.0"
  local cargo_pre="3.0.0-beta.1" wheel_pre="3.0.0b1"
  local py_name="pulsehive" stale="0.3.0b2" plain_out=""

  # 0a. The class B / round-2 rule itself, before any candidate set is judged:
  #     the spellings a `v*` tag and a maturin-built wheel can use for one
  #     version must compare equal, a real version difference must not be
  #     normalized away, and nothing here may be decided by how a value would
  #     read as a number (`1.10` and `1.1` are different versions, [1, 10] vs
  #     [1, 1], though they are the same number; `1e2` is not a version, though
  #     `100` is one). A rule that always said "equal", or compared numbers,
  #     fails below.
  expect_eq() { # <a> <b>
    version_eq "$1" "$2" || {
      echo "self-test: FAILED [version rule]: '$1' and '$2' are the same PEP 440 version but compared unequal" >&2
      exit 1
    }
  }
  expect_ne() { # <a> <b>
    if version_eq "$1" "$2"; then
      echo "self-test: FAILED [version rule]: '$1' and '$2' are different versions but compared equal" >&2
      exit 1
    fi
  }
  expect_eq "3.0.0-beta.1" "3.0.0b1"
  expect_eq "3.0.0b1" "3.0.0-beta.1"
  expect_eq "3.0.0-beta.1" "3.0.0-BETA.1"
  expect_eq "3.0.0-alpha.2" "3.0.0a2"
  expect_eq "3.0.0-rc.3" "3.0.0rc3"
  expect_eq "3.0.0" "3.0.0"
  expect_eq "1.0" "1.00"
  expect_eq "1.0" "1.0.0"
  expect_eq "1.0-1" "1.0.post1"
  expect_ne "1.10" "1.1"
  expect_ne "1.10" "1.1.0"
  expect_ne "1e2" "100"
  expect_ne "3.0.0b1" "3.0.0b2"
  expect_ne "3.0.0" "3.0.0b1"
  expect_ne "3.0.0b1" "3.0.0rc1"
  expect_ne "0.3.0b2" "3.0.0"

  # 0. The probe's endpoint and its three outcomes. The per-version endpoint is
  #    what is asked, in canonical PEP 440 spelling; 200 means published, 404
  #    means not published (an absent project reads the same way, so a
  #    first-ever publish is allowed through), and everything else — any other
  #    status, no status, or a dead connection — fails closed.
  expect_url "per-version endpoint" "$PYPI_JSON_URL/pulsehive/$ver/json" pulsehive "$ver"
  expect_url "per-version endpoint, prerelease normalized" "$PYPI_JSON_URL/pulsehive/$wheel_pre/json" pulsehive "$cargo_pre"
  expect_url "per-version endpoint, distribution name normalized" "$PYPI_JSON_URL/pulsehive/$ver/json" "PulseHive" "$ver"
  expect_url "per-version endpoint, separators collapsed" "$PYPI_JSON_URL/pulsehive-sdk/$ver/json" "pulsehive__sdk" "$ver"
  expect_status "published version (HTTP 200)" pulsehive "$ver" 0
  SELFTEST_HTTP_CODE="404"
  expect_status "unpublished version (HTTP 404)" pulsehive "$ver" 1
  SELFTEST_HTTP_CODE="401"
  expect_status "unexpected status (HTTP 401) fails closed" pulsehive "$ver" 2
  SELFTEST_HTTP_CODE="500"
  expect_status "unexpected status (HTTP 500) fails closed" pulsehive "$ver" 2
  SELFTEST_HTTP_CODE="000"
  expect_status "no status code at all fails closed" pulsehive "$ver" 2
  SELFTEST_HTTP_FAIL=1
  expect_status "transport failure fails closed" pulsehive "$ver" 2
  SELFTEST_HTTP_FAIL=0
  SELFTEST_HTTP_CODE="404"
  expect_status "version with no release segment fails closed" pulsehive "not-a-version" 2

  # Fixture wheels are throwaway zips carrying a minimal *.dist-info/METADATA:
  # the identity chain (V2) reads the artifact's own claim, so an empty file
  # could never stand in for a wheel. The metadata pair defaults to the
  # filename's own fields; the two extra arguments plant a disagreement.
  make_wheel() { # <dir> <fname-name> <fname-version> <py> <abi> <platform> [<meta-name> <meta-version>]
    python3 - "$@" <<'PLANT_WHEEL'
import os
import sys
import zipfile

args = sys.argv[1:]
directory, filename_name, filename_version, py_tag, abi_tag, platform = args[:6]
meta_name = args[6] if len(args) > 6 else filename_name
meta_version = args[7] if len(args) > 7 else filename_version
os.makedirs(directory, exist_ok=True)
wheel = os.path.join(directory, f"{filename_name}-{filename_version}-{py_tag}-{abi_tag}-{platform}.whl")
dist_info = f"{filename_name.replace('-', '_')}-{meta_version}.dist-info"
with zipfile.ZipFile(wheel, "w") as archive:
    archive.writestr(
        f"{dist_info}/METADATA",
        f"Metadata-Version: 2.1\nName: {meta_name}\nVersion: {meta_version}\n",
    )
PLANT_WHEEL
  }
  make_set() { # a correctly-formed candidate set: one cp311-abi3 wheel per target
    mkdir -p "$1"
    make_wheel "$1" pulsehive "$ver" cp311 abi3 macosx_11_0_arm64
    make_wheel "$1" pulsehive "$ver" cp311 abi3 manylinux_2_28_x86_64
    make_wheel "$1" pulsehive "$ver" cp311 abi3 win_amd64
  }
  # A PyPI per-version body for the wheels in <dist-dir>:
  #   complete — every wheel, each with its real sha256
  #   partial  — every wheel but the last (sorted)
  #   differ   — every wheel, the first carrying a sha256 that is not its own
  #   yanked   — every wheel, the first flagged yanked (its sha256 stays real)
  #   extra    — every wheel plus a file this set does not contain
  pypi_body() { # <out-file> <mode> <dist-dir>
    python3 - "$@" <<'PLANT_BODY'
import glob
import hashlib
import json
import os
import sys

out, mode, dist = sys.argv[1], sys.argv[2], sys.argv[3]
wheels = sorted(glob.glob(os.path.join(dist, "*.whl")))


def sha256(path):
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 16), b""):
            digest.update(chunk)
    return digest.hexdigest()


def entry(path, yanked=False):
    item = {"filename": os.path.basename(path), "digests": {"sha256": sha256(path)}}
    if yanked:
        item["yanked"] = True
    return item


if mode == "complete":
    urls = [entry(w) for w in wheels]
elif mode == "partial":
    urls = [entry(w) for w in wheels[:-1]]
elif mode == "differ":
    urls = [entry(w) for w in wheels]
    urls[0]["digests"]["sha256"] = "0" * 64
elif mode == "yanked":
    urls = [entry(w, yanked=(index == 0)) for index, w in enumerate(wheels)]
elif mode == "extra":
    urls = [entry(w) for w in wheels]
    urls.append({
        "filename": "pulsehive-3.0.0-cp311-abi3-musllinux_1_2_x86_64.whl",
        "digests": {"sha256": "1" * 64},
    })
else:
    sys.exit(2)
with open(out, "w", encoding="utf-8") as handle:
    json.dump({"urls": urls}, handle)
PLANT_BODY
  }
  make_body() { # <label> <mode> <dist-dir> — a body file named for its case
    pypi_body "$tmp/body-$1.json" "$2" "$3"
  }

  expect_ok() { # <label> <dir> <expect-version> <expect-name>
    local out rc
    out=$(check_dist "$2" "$3" "$4" 2>&1)
    rc=$?
    if [ "$rc" -ne 0 ]; then
      echo "self-test: FAILED [$1]: expected publishable, got rc=$rc: $out" >&2
      exit 1
    fi
  }
  expect_reject() { # <label> <dir> <expect-version> <expect-name> <named-error substring>
    local out rc
    out=$(check_dist "$2" "$3" "$4" 2>&1)
    rc=$?
    if [ "$rc" -eq 0 ]; then
      echo "self-test: FAILED [$1]: expected rejection, but the check exited 0" >&2
      exit 1
    fi
    case "$out" in
      *"$5"*) : ;;
      *)
        echo "self-test: FAILED [$1]: rejected (rc=$rc) but without the named error ('$5'): $out" >&2
        exit 1
        ;;
    esac
  }

  # The r2.s3.w4 cases name their outcome on stdout — `case <label>: <outcome>` —
  # so each new behaviour is visible in the transcript, not only counted.
  case_rejected() { # <label> <dir> <expect-version> <expect-name> <named-error>
    local out rc
    out=$(check_dist "$2" "$3" "$4" 2>&1)
    rc=$?
    if [ "$rc" -eq 0 ]; then
      echo "self-test: FAILED [$1]: expected rejection, but the check exited 0" >&2
      exit 1
    fi
    case "$out" in
      *"$5"*) : ;;
      *)
        echo "self-test: FAILED [$1]: rejected (rc=$rc) but without the named error ('$5'): $out" >&2
        exit 1
        ;;
    esac
    echo "case $1: rejected"
  }
  case_mode() { # <label> <expected-mode> <dir> <expect-version> <expect-name> [<expected substring>]
    local out rc
    out=$(check_dist "$3" "$4" "$5" 2>&1)
    rc=$?
    if [ "$rc" -ne 0 ]; then
      echo "self-test: FAILED [$1]: expected publish-mode $2, got rc=$rc: $out" >&2
      exit 1
    fi
    case "$out" in
      *"publish-mode: $2"*) : ;;
      *)
        echo "self-test: FAILED [$1]: expected 'publish-mode: $2', got: $out" >&2
        exit 1
        ;;
    esac
    if [ -n "${6:-}" ]; then
      case "$out" in
        *"$6"*) : ;;
        *)
          echo "self-test: FAILED [$1]: accepted as $2 but without '$6': $out" >&2
          exit 1
          ;;
      esac
    fi
    echo "case $1: $2"
  }
  case_verify_ok() { # <label> <dir> <expect-version> <expect-name>
    local out rc
    out=$(verify_published_files "$2" "$3" "$4" 2>&1)
    rc=$?
    if [ "$rc" -ne 0 ]; then
      echo "self-test: FAILED [$1]: expected a verified set, got rc=$rc: $out" >&2
      exit 1
    fi
    case "$out" in
      *"verify-published: ok"*) : ;;
      *)
        echo "self-test: FAILED [$1]: no 'verify-published: ok' line: $out" >&2
        exit 1
        ;;
    esac
    echo "case $1: accepted"
  }
  # case_refuse — a fail-closed refusal must still announce its mode: §4.1.4's
  # "always printed", on stdout and in $GITHUB_OUTPUT, right before the named
  # error. The refusal paths that decide the state cannot be read are exactly
  # where the mode matters most, so each one is asserted, not implied by the
  # violation path's behaviour.
  case_refuse() { # <label> <dir> <expect-version> <expect-name> <named-error>
    local out rc gh
    gh="$tmp/gh-output-$1.txt"
    : >"$gh"
    out=$(GITHUB_OUTPUT="$gh" check_dist "$2" "$3" "$4" 2>&1)
    rc=$?
    if [ "$rc" -eq 0 ]; then
      echo "self-test: FAILED [$1]: expected a refusal, but the check exited 0: $out" >&2
      exit 1
    fi
    case "$out" in
      *"$5"*) : ;;
      *)
        echo "self-test: FAILED [$1]: refused (rc=$rc) but without the named error ('$5'): $out" >&2
        exit 1
        ;;
    esac
    case "$out" in
      *"publish-mode: refuse"*) : ;;
      *)
        echo "self-test: FAILED [$1]: refused without printing 'publish-mode: refuse' (spec §4.1.4): $out" >&2
        exit 1
        ;;
    esac
    grep -qxF 'publish-mode=refuse' "$gh" || {
      echo "self-test: FAILED [$1]: \$GITHUB_OUTPUT did not receive 'publish-mode=refuse' (spec §4.1.4)" >&2
      exit 1
    }
    echo "case $1: refuse"
  }
  case_verify_rejected() { # <label> <dir> <expect-version> <expect-name> <named-error>
    local out rc
    out=$(verify_published_files "$2" "$3" "$4" 2>&1)
    rc=$?
    if [ "$rc" -eq 0 ]; then
      echo "self-test: FAILED [$1]: expected a failed verification, but it exited 0" >&2
      exit 1
    fi
    case "$out" in
      *"$5"*) : ;;
      *)
        echo "self-test: FAILED [$1]: failed (rc=$rc) but without the named error ('$5'): $out" >&2
        exit 1
        ;;
    esac
    echo "case $1: rejected"
  }

  # 1. A correctly-formed, unpublished set passes (stub: the per-version
  #    endpoint answers 404, which is also what a project that has never been
  #    published answers — hence the check below that the probe really asked
  #    for the endpoint it claims to).
  SELFTEST_HTTP_CODE="404"
  SELFTEST_HTTP_BODY=""
  d="$tmp/pass"
  make_set "$d"
  expect_ok "correctly-formed set (first-ever publish)" "$d" "$ver" "$py_name"
  grep -qxF "$PYPI_JSON_URL/pulsehive/$ver/json" "$SELFTEST_HTTP_LOG" || {
    echo "self-test: FAILED [probe endpoint]: the gate never asked the per-version endpoint for '$ver'" >&2
    exit 1
  }

  # 2. An advertised target's wheel missing from the set.
  d="$tmp/missing"
  make_set "$d"
  rm "$d/pulsehive-$ver-cp311-abi3-win_amd64.whl"
  expect_reject "missing target" "$d" "$ver" "$py_name" \
    "missing wheel for advertised target 'x86_64-pc-windows-msvc'"

  # 3. A wheel whose platform tag does not match any advertised target is named
  # as an excluded host (A6/V4), not as a generic mismatch.
  d="$tmp/mislabelled"
  make_set "$d"
  mv "$d/pulsehive-$ver-cp311-abi3-macosx_11_0_arm64.whl" \
    "$d/pulsehive-$ver-cp311-abi3-sunos_x86.whl"
  expect_reject "mislabelled target" "$d" "$ver" "$py_name" \
    "is built for an excluded host ('sunos_x86'); advertised targets: aarch64-apple-darwin x86_64-unknown-linux-gnu x86_64-pc-windows-msvc"

  # 4. A wheel whose version disagrees with --expect-version.
  d="$tmp/version"
  make_set "$d"
  mv "$d/pulsehive-$ver-cp311-abi3-win_amd64.whl" \
    "$d/pulsehive-0.3.0b2-cp311-abi3-win_amd64.whl"
  expect_reject "version disagreement" "$d" "$ver" "$py_name" \
    "carries version '0.3.0b2' but --expect-version is '$ver'"

  # 5. A 200 whose body cannot be read is not an acceptance and not the old
  #    "already published" refusal either (the published-state classification
  #    replaced it): the state cannot be proved, so the gate fails closed.
  SELFTEST_HTTP_CODE="200"
  d="$tmp/published"
  make_set "$d"
  expect_reject "published version with an unreadable body fails closed" "$d" "$ver" "$py_name" \
    "cannot read the per-version JSON for 'pulsehive' '$ver' — refusing to publish over a state that cannot be proved"

  # 5b. ...and a probe that cannot establish the state (stub: HTTP 503)
  # refuses to publish rather than assuming the version is free.
  SELFTEST_HTTP_CODE="503"
  expect_reject "unverifiable PyPI state" "$d" "$ver" "$py_name" "cannot verify PyPI state"

  # 6. A prerelease candidate set: the wheels carry maturin's spelling
  # (`3.0.0b1`) while --expect-version arrives as the tag's Cargo spelling
  # (`3.0.0-beta.1`). Those name the same version, so the set is publishable.
  SELFTEST_HTTP_CODE="404"
  d="$tmp/prerelease"
  mkdir -p "$d"
  make_wheel "$d" pulsehive "$wheel_pre" cp311 abi3 macosx_11_0_arm64
  make_wheel "$d" pulsehive "$wheel_pre" cp311 abi3 manylinux_2_28_x86_64
  make_wheel "$d" pulsehive "$wheel_pre" cp311 abi3 win_amd64
  expect_ok "prerelease set against the tag's Cargo spelling" "$d" "$cargo_pre" "$py_name"

  # 7. ...and the normalization must not turn a different prerelease into the
  # same version: `3.0.0b1` wheels are not `3.0.0-beta.2`.
  expect_reject "prerelease set against a different prerelease" "$d" "3.0.0-beta.2" "$py_name" \
    "carries version '$wheel_pre' but --expect-version is '3.0.0-beta.2'"

  # 8. Wheels only: any other file in dist/ is a named rejection rather than
  # something the uploader has to notice — including a dotfile, which is
  # exactly the kind of stray a macOS build agent leaves behind.
  SELFTEST_HTTP_CODE="404"
  d="$tmp/stray"
  make_set "$d"
  : > "$d/pulsehive-$ver.tar.gz"
  expect_reject "stray tarball in dist" "$d" "$ver" "$py_name" "non-wheel artifact(s) in '$d': pulsehive-$ver.tar.gz"

  d="$tmp/stray-hidden"
  make_set "$d"
  : > "$d/.DS_Store"
  expect_reject "stray dotfile in dist" "$d" "$ver" "$py_name" "non-wheel artifact(s) in '$d': .DS_Store"

  # 8b. An entry that is not a regular file is rejected too, not skipped: a
  # directory in dist/ is what the old `[ -f "$entry" ] || continue` walked
  # past, and a directory whose name is a perfectly good wheel filename is what
  # a name-only test would let through. Both are proven, and the fixtures are
  # checked so a planting failure can never read as an acceptance.
  d="$tmp/stray-dir"
  make_set "$d"
  mkdir -p "$d/leftover-dir" || fail "self-test: cannot plant a directory in dist"
  expect_reject "directory in dist" "$d" "$ver" "$py_name" "non-wheel artifact(s) in '$d': leftover-dir/"

  d="$tmp/stray-dir-named-whl"
  make_set "$d"
  rm "$d/pulsehive-$ver-cp311-abi3-win_amd64.whl"
  mkdir -p "$d/pulsehive-$ver-cp311-abi3-win_amd64.whl" ||
    fail "self-test: cannot plant a wheel-named directory in dist"
  expect_reject "directory named like a wheel" "$d" "$ver" "$py_name" \
    "non-wheel artifact(s) in '$d': pulsehive-$ver-cp311-abi3-win_amd64.whl/"

  # ...and a symlink is not a regular file either. Only run where the host
  # really makes symlinks (Git Bash on Windows may copy instead), so the case
  # proves the rule rather than the filesystem's mood.
  d="$tmp/stray-symlink"
  make_set "$d"
  mv "$d/pulsehive-$ver-cp311-abi3-macosx_11_0_arm64.whl" "$tmp/real-wheel.whl" ||
    fail "self-test: cannot move a wheel aside for the symlink case"
  if ln -s "$tmp/real-wheel.whl" "$d/pulsehive-$ver-cp311-abi3-macosx_11_0_arm64.whl" 2>/dev/null &&
    [ -L "$d/pulsehive-$ver-cp311-abi3-macosx_11_0_arm64.whl" ]; then
    expect_reject "symlink in dist" "$d" "$ver" "$py_name" \
      "non-wheel artifact(s) in '$d': pulsehive-$ver-cp311-abi3-macosx_11_0_arm64.whl"
  fi

  # 9. The r2.s3.w4 judgements. Each new case names its outcome on stdout, and
  #    each drives the real check: only the HTTP layer is stubbed, so the body a
  #    case serves is read by the same code the release path runs.

  # 9a. #107: the filename's distribution name is checked, not merely read.
  d="$tmp/foreign-distribution"
  make_set "$d"
  rm "$d/pulsehive-$ver-cp311-abi3-win_amd64.whl"
  make_wheel "$d" pulsehive_sdk "$ver" cp311 abi3 win_amd64
  case_rejected "foreign-distribution" "$d" "$ver" "$py_name" \
    "belongs to distribution 'pulsehive_sdk', not 'pulsehive'"

  # 9b. V2: the wheel's METADATA Name must be the distribution --expect-name
  #     declares, in PEP 503's normalized form.
  d="$tmp/metadata-name-mismatch"
  make_set "$d"
  rm "$d/pulsehive-$ver-cp311-abi3-macosx_11_0_arm64.whl"
  make_wheel "$d" pulsehive "$ver" cp311 abi3 macosx_11_0_arm64 pulsehive_sdk "$ver"
  case_rejected "metadata-name-mismatch" "$d" "$ver" "$py_name" \
    "METADATA says Name 'pulsehive_sdk', expected 'pulsehive'"

  # 9c. V2: the wheel's METADATA Version must be the version --expect-version
  #     names, compared as PEP 440 versions.
  d="$tmp/metadata-version-mismatch"
  make_set "$d"
  rm "$d/pulsehive-$ver-cp311-abi3-win_amd64.whl"
  make_wheel "$d" pulsehive "$ver" cp311 abi3 win_amd64 pulsehive "$stale"
  case_rejected "metadata-version-mismatch" "$d" "$ver" "$py_name" \
    "METADATA says Version '$stale', expected '$ver'"

  # 9d. A6/V4: a musllinux wheel is built for a host the release does not
  #     advertise and is named as an excluded host.
  d="$tmp/excluded-host"
  make_set "$d"
  rm "$d/pulsehive-$ver-cp311-abi3-manylinux_2_28_x86_64.whl"
  make_wheel "$d" pulsehive "$ver" cp311 abi3 musllinux_1_2_x86_64
  case_rejected "excluded-host" "$d" "$ver" "$py_name" \
    "is built for an excluded host ('musllinux_1_2_x86_64'); advertised targets: aarch64-apple-darwin x86_64-unknown-linux-gnu x86_64-pc-windows-msvc"

  # 9e. A 200 whose body proves every published file is a tested wheel with the
  #     same sha256 and every tested wheel is published: complete — nothing to
  #     upload.
  d="$tmp/complete-release"
  make_set "$d"
  make_body "complete" complete "$d"
  SELFTEST_HTTP_CODE="200"
  SELFTEST_HTTP_BODY="$tmp/body-complete.json"
  case_mode "complete-release" complete "$d" "$ver" "$py_name" "wheel-sha256:"

  # 9f. A 200 that holds some of the tested wheels: resume, with the missing
  #     wheels listed so the re-run's upload set is visible before approval.
  d="$tmp/partial-release"
  make_set "$d"
  make_body "partial" partial "$d"
  SELFTEST_HTTP_CODE="200"
  SELFTEST_HTTP_BODY="$tmp/body-partial.json"
  case_mode "partial-release" resume "$d" "$ver" "$py_name" \
    "missing wheel(s): pulsehive-$ver-cp311-abi3-win_amd64.whl"

  # 9g. A published file carrying a tested wheel's name and different bytes:
  #     refuse. The gate must never upload over bytes it cannot account for.
  d="$tmp/published-bytes-differ"
  make_set "$d"
  make_body "differ" differ "$d"
  SELFTEST_HTTP_CODE="200"
  SELFTEST_HTTP_BODY="$tmp/body-differ.json"
  case_rejected "published-bytes-differ" "$d" "$ver" "$py_name" \
    "differs from the tested wheel — refusing"
  # ...and the fresh-build hint stays out of that message unless --fresh-build
  # was passed: a tag re-run's recovery is to re-run the failed jobs, not to
  # rebuild and supersede.
  plain_out=$(check_dist "$d" "$ver" "$py_name" 2>&1) || true
  case "$plain_out" in
    *"this is a fresh build"*)
      echo "self-test: FAILED [published-bytes-differ]: the fresh-build hint appeared without --fresh-build" >&2
      exit 1
      ;;
  esac

  # 9h. A published file this set does not contain: refuse.
  d="$tmp/published-extra-file"
  make_set "$d"
  make_body "extra" extra "$d"
  SELFTEST_HTTP_CODE="200"
  SELFTEST_HTTP_BODY="$tmp/body-extra.json"
  case_rejected "published-extra-file" "$d" "$ver" "$py_name" \
    "is not in the tested set — refusing"

  # 9i. A17: a published file marked yanked refuses even though its bytes match
  #     the tested wheel — yanked is not complete, and unyanking is the
  #     operator's decision (docs/RELEASING.md).
  d="$tmp/yanked-file"
  make_set "$d"
  make_body "yanked" yanked "$d"
  SELFTEST_HTTP_CODE="200"
  SELFTEST_HTTP_BODY="$tmp/body-yanked.json"
  case_rejected "yanked-file" "$d" "$ver" "$py_name" \
    "is yanked on PyPI — not complete; unyanking is the operator's decision (docs/RELEASING.md)"

  # 9j. L4 narrowed: with --fresh-build (the workflow passes it on
  #     workflow_dispatch) the differs refusal tells the operator the recovery.
  d="$tmp/fresh-build-hint"
  make_set "$d"
  make_body "fresh" differ "$d"
  SELFTEST_HTTP_CODE="200"
  SELFTEST_HTTP_BODY="$tmp/body-fresh.json"
  FRESH_BUILD=1
  case_rejected "fresh-build-hint" "$d" "$ver" "$py_name" \
    "this is a fresh build; re-run the failed jobs of the original tag run instead"
  FRESH_BUILD=0

  # 9k. --verify-published waits for PyPI's index: the first read answers 404
  #     and the retry proves all three files, so the retry is what makes this
  #     pass. The call counter proves the first read really happened.
  d="$tmp/verify-published-ok"
  make_set "$d"
  make_body "verify-ok" complete "$d"
  SELFTEST_HTTP_CODE="404"
  SELFTEST_HTTP_BODY=""
  : >"$SELFTEST_HTTP_LOG" # this case counts its own reads, so read 2 is read 2
  verify_after_first_read() {
    if [ "$SELFTEST_HTTP_CALLS" -ge 2 ]; then
      SELFTEST_HTTP_CODE="200"
      SELFTEST_HTTP_BODY="$tmp/body-verify-ok.json"
    fi
  }
  SELFTEST_HTTP_HOOK="verify_after_first_read"
  case_verify_ok "verify-published-ok" "$d" "$ver" "$py_name"
  SELFTEST_HTTP_HOOK=""
  verify_reads=$(grep -c '' "$SELFTEST_HTTP_LOG")
  [ "$verify_reads" -ge 2 ] || {
    echo "self-test: FAILED [verify-published-ok]: no retry happened ($verify_reads read(s))" >&2
    exit 1
  }

  # 9l. ...and a tested wheel the upload did not land fails, naming the file.
  d="$tmp/verify-published-missing"
  make_set "$d"
  make_body "verify-missing" partial "$d"
  SELFTEST_HTTP_CODE="200"
  SELFTEST_HTTP_BODY="$tmp/body-verify-missing.json"
  case_verify_rejected "verify-published-missing" "$d" "$ver" "$py_name" \
    "wheel 'pulsehive-$ver-cp311-abi3-win_amd64.whl' is not on PyPI"

  # 9m. The fail-closed refusals announce their mode too (§4.1.4's "always
  #     printed"): a state that cannot be read refuses with `publish-mode:
  #     refuse` on stdout and `publish-mode=refuse` in $GITHUB_OUTPUT, so the
  #     workflow's summary can say what the job refused to do.
  d="$tmp/refuse-unreadable-body"
  make_set "$d"
  SELFTEST_HTTP_CODE="200"
  SELFTEST_HTTP_BODY=""
  case_refuse "refuse-unreadable-body" "$d" "$ver" "$py_name" \
    "cannot read the per-version JSON for 'pulsehive' '$ver'"

  d="$tmp/refuse-unverifiable-state"
  make_set "$d"
  SELFTEST_HTTP_CODE="503"
  SELFTEST_HTTP_BODY=""
  case_refuse "refuse-unverifiable-state" "$d" "$ver" "$py_name" \
    "cannot verify PyPI state for 'pulsehive' '$ver'"

  echo "self-test: ok"
}

# need_args <required> <remaining> — the CLI's arity guard: usage and exit 2
# unless <remaining> (the caller's own $#) is at least <required>, the flag
# itself counting. A function cannot see its caller's $#, so the caller passes
# it; that is the whole reason this is not an inline test.
need_args() { # <required> <remaining>
  [ "$1" -le "$2" ] || {
    usage >&2
    exit 2
  }
}

# dispatch_probe <project> <version> — the read-only probe's own validation, its
# scratch dir, and its run.
dispatch_probe() { # <project> <version>
  if [ -z "$1" ] || [ -z "$2" ]; then
    usage >&2
    echo "$PROG: --probe needs both <project> and <version>" >&2
    exit 2
  fi
  WORK_DIR=$(mktemp -d "${TMPDIR:-/tmp}/py-publish-check.XXXXXX") ||
    fail "cannot create a temp dir for the probe"
  trap 'rm -rf "${WORK_DIR:-}"' EXIT
  probe "$1" "$2"
}

# dispatch_dist <dist-dir> <expect-version> <expect-name> <verify 0|1> — the
# --dist path's own validation, its scratch dir, and the one call its mode needs.
dispatch_dist() { # <dist-dir> <expect-version> <expect-name> <verify 0|1>
  if [ -z "$1" ]; then
    usage >&2
    echo "$PROG: either --self-test, --probe <project> <version>, or --dist <dir> with --expect-name and --expect-version, is required" >&2
    exit 2
  fi
  if [ -z "$2" ] || [ -z "$3" ]; then
    usage >&2
    echo "$PROG: --dist requires --expect-name <name> (#107) and --expect-version <version>" >&2
    exit 2
  fi
  WORK_DIR=$(mktemp -d "${TMPDIR:-/tmp}/py-publish-check.XXXXXX") ||
    fail "cannot create a temp dir"
  trap 'rm -rf "${WORK_DIR:-}"' EXIT
  if [ "$4" -eq 1 ]; then
    verify_published_files "$1" "$2" "$3"
  else
    check_dist "$1" "$2" "$3"
  fi
}

main() {
  local dist_dir="" expect_version="" expect_name="" self_test=0 want_verify=0
  local probe_project="" probe_version=""
  FRESH_BUILD=0
  while [ $# -gt 0 ]; do
    case "$1" in
      --dist)
        need_args 2 "$#"
        dist_dir="$2"
        shift 2
        ;;
      --expect-version)
        need_args 2 "$#"
        expect_version="$2"
        shift 2
        ;;
      --expect-name)
        need_args 2 "$#"
        expect_name="$2"
        shift 2
        ;;
      --verify-published)
        want_verify=1
        shift
        ;;
      --fresh-build)
        FRESH_BUILD=1
        shift
        ;;
      --probe)
        need_args 3 "$#"
        probe_project="$2"
        probe_version="$3"
        shift 3
        ;;
      --self-test)
        self_test=1
        shift
        ;;
      -h | --help)
        usage
        exit 0
        ;;
      *)
        echo "$PROG: unknown argument '$1'" >&2
        usage >&2
        exit 2
        ;;
    esac
  done

  if [ "$self_test" -eq 1 ]; then
    self_test
    exit 0
  fi

  if [ -n "$probe_project" ] || [ -n "$probe_version" ]; then
    dispatch_probe "$probe_project" "$probe_version"
    return
  fi

  dispatch_dist "$dist_dir" "$expect_version" "$expect_name" "$want_verify"
}

SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
[ -r "$SCRIPT_DIR/lib/pep440.sh" ] ||
  fail "missing the version rule $SCRIPT_DIR/lib/pep440.sh (it ships next to this script)"
# shellcheck source=lib/pep440.sh
. "$SCRIPT_DIR/lib/pep440.sh"

main "$@"
