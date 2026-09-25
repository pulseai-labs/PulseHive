#!/usr/bin/env bash
# py-publish-check.sh — fail-closed pre-publish gate for the `pulsehive` Python
# distribution (r2.s2.w2). RELEASE.md's negative control for the release path;
# demo line d24.
#
# Modes:
#   --dist <dir> --expect-version <version>
#       Verify that <dir> holds a complete, correctly-labelled, correctly-
#       versioned wheel set covering every advertised target, and that
#       <version> is NOT already published on PyPI. The state is asked of the
#       JSON API's per-version endpoint and decided by its HTTP status: 200 is
#       "published" (a hard failure, never a warning), 404 is "not published"
#       — which is also what an absent project answers, so a first-ever
#       publish is allowed through — and any other status, or a transport
#       failure, refuses to publish (fail closed). Every rejection exits
#       non-zero with a named error on stderr; a publishable set prints a
#       one-line summary and exits 0.
#
#       <version> arrives as the `v*` tag's spelling of the version the
#       manifest declares, while the wheel filenames carry maturin's PEP 440
#       spelling of it; the two are compared as versions through
#       lib/pep440.sh, so a prerelease tag such as `v3.0.0-beta.1` matches the
#       `3.0.0b1` wheels it names instead of being refused for its spelling.
#   --self-test
#       Hermetic (no network, no credentials): stubs only the HTTP layer,
#       builds throwaway candidate sets under a temp dir, and asserts that
#       every rejection fires AND that a correctly-formed set passes — the
#       four wheel-set rejections (missing target, mislabelled target, version
#       disagreement, already published), the non-wheel-artifact rejection,
#       the version rule including the prerelease spellings a tag and maturin
#       disagree on, and the published-state probe's three outcomes (200, 404,
#       and everything else failing closed). Prints `self-test: ok` as its
#       last line on success.
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
  $PROG --dist <dir> --expect-version <version>
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
#   `3.0.0b1`. Pure and network-free: --self-test judges the URL a probe would
#   use, and rc non-zero means the version has no release segment to spell.
pypi_version_url() {
  local canon
  canon=$(pep440_canon "$2") || return 1
  printf '%s/%s/%s/json' "$PYPI_JSON_URL" "$1" "$canon"
}

# pypi_http_status <url>
#   The transport: the HTTP status code a GET of <url> answers with, or rc
#   non-zero when the request itself failed (DNS, TLS, timeout). Deliberately
#   without curl -f: here the status IS the answer, so a 404 has to stay
#   readable instead of being flattened into a transport error.
pypi_http_status() {
  curl -s -o /dev/null -w '%{http_code}' --max-time 30 "$1" 2>/dev/null
}

# pypi_version_status <project> <version>
#   rc 0 — <version> is published for <project> (HTTP 200)
#   rc 1 — <version> is not published (HTTP 404; an absent project answers with
#          the same 404, which is why a first-ever publish is allowed through)
#   rc 2 — the state could not be established: any other status, an
#          unspellable version, or a transport failure. Callers fail closed.
pypi_version_status() {
  local project="$1" version="$2" url code
  url=$(pypi_version_url "$project" "$version") || return 2
  code=$(pypi_http_status "$url") || return 2
  case "$code" in
    200) return 0 ;;
    404) return 1 ;;
    *) return 2 ;;
  esac
}

check_dist() {
  local dist_dir="$1" expect_version="$2"
  [ -d "$dist_dir" ] || fail "dist directory '$dist_dir' not found"

  # The release publishes wheels only, and the publish action uploads whatever
  # is in dist/ — with the sdist build gone, nothing else rejects a stray file
  # (a leftover tarball, a zip, an editor backup, a build log), which would
  # otherwise be uploaded as an artifact no check ever looked at.
  local wheels=() others=() entry
  shopt -s nullglob dotglob
  wheels=("$dist_dir"/*.whl)
  for entry in "$dist_dir"/*; do
    [ -f "$entry" ] || continue
    case "${entry##*/}" in
      *.whl) : ;;
      *) others+=("${entry##*/}") ;;
    esac
  done
  shopt -u nullglob dotglob
  [ "${#wheels[@]}" -gt 0 ] || fail "no wheels (*.whl) found in '$dist_dir'"
  [ "${#others[@]}" -eq 0 ] ||
    fail "non-wheel artifact(s) in '$dist_dir': ${others[*]} — the release publishes wheels (*.whl) only"

  local wheel base version platform name="" matched_target target pattern
  local seen=" "
  for wheel in "${wheels[@]}"; do
    base="${wheel##*/}"
    base="${base%.whl}"
    local fields=()
    IFS='-' read -r -a fields <<< "$base"
    if [ "${#fields[@]}" -ne 5 ]; then
      fail "mislabelled wheel '$base': filename is not <name>-<version>-<python>-<abi>-<platform>"
    fi
    name="${fields[0]}"
    version="${fields[1]}"
    platform="${fields[4]}"
    matched_target=""
    for target in "${TARGETS[@]}"; do
      pattern=$(target_platform_pattern "$target") ||
        fail "internal: no platform pattern for target '$target'"
      if printf '%s' "$platform" | grep -qE "^(${pattern})\$"; then
        matched_target="$target"
        seen="$seen$target "
        break
      fi
    done
    [ -n "$matched_target" ] ||
      fail "mislabelled wheel '$base': platform tag '$platform' does not match any advertised target"
    version_eq "$version" "$expect_version" ||
      fail "version disagreement: wheel '$base' carries version '$version' but --expect-version is '$expect_version' (compared as PEP 440 versions)"
  done

  for target in "${TARGETS[@]}"; do
    case "$seen" in
      *" $target "*) : ;;
      *) fail "missing wheel for advertised target '$target' (looked in '$dist_dir')" ;;
    esac
  done

  local rc=0
  pypi_version_status "$name" "$expect_version" || rc=$?
  case $rc in
    0) fail "already published: version '$expect_version' of project '$name' already exists on PyPI — re-publishing an existing version is forbidden" ;;
    1) : ;;
    *) fail "cannot verify PyPI state for '$name' '$expect_version' (the per-version JSON probe answered neither 200 nor 404) — refusing to publish an unproven version" ;;
  esac

  echo "$PROG: ok: ${#wheels[@]} wheel(s) cover all ${#TARGETS[@]} advertised targets at version '$expect_version' (project '$name') — publishable"
}

self_test() {
  # tmp is deliberately global so the EXIT trap below can still see it
  tmp=""
  tmp=$(mktemp -d "${TMPDIR:-/tmp}/py-publish-check-selftest.XXXXXX") ||
    fail "self-test: cannot create a temp dir"
  trap 'rm -rf "${tmp:-}"' EXIT

  # Hermetic: stub ONLY the HTTP layer. The endpoint a probe asks for and the
  # rc mapping that turns a status code into published / not published / cannot
  # tell both run for real, so a wrong endpoint or a wrong reading of a status
  # fails here offline instead of on the release path, where the answers are
  # live. The stub records every URL it is handed and can fail like a dead
  # network.
  SELFTEST_HTTP_LOG="$tmp/http-requests.log"
  : > "$SELFTEST_HTTP_LOG"
  SELFTEST_HTTP_CODE="200"
  SELFTEST_HTTP_FAIL=0
  pypi_http_status() { # <url>
    printf '%s\n' "$1" >> "$SELFTEST_HTTP_LOG"
    if [ "${SELFTEST_HTTP_FAIL:-0}" -eq 1 ]; then
      return 7
    fi
    printf '%s' "$SELFTEST_HTTP_CODE"
  }
  expect_status() { # <label> <project> <version> <expected rc>
    local rc=0
    pypi_version_status "$2" "$3" || rc=$?
    [ "$rc" -eq "$4" ] || {
      echo "self-test: FAILED [$1]: pypi_version_status '$2' '$3' -> rc=$rc, expected rc=$4" >&2
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

  # 0a. The class B rule itself, before any candidate set is judged: the
  #     spellings a `v*` tag and a maturin-built wheel can use for one version
  #     must compare equal, and a real version difference must not be
  #     normalized away (a rule that always said "equal" would pass every
  #     acceptance case below without proving anything).
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
  expect_eq "1.0-1" "1.0.post1"
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

  make_wheel() { # <dir> <name> <version> <pytag> <abitag> <platform>
    printf '' > "$1/$2-$3-$4-$5-$6.whl"
  }
  make_set() { # a correctly-formed candidate set: one cp311-abi3 wheel per target
    mkdir -p "$1"
    make_wheel "$1" pulsehive "$ver" cp311 abi3 macosx_11_0_arm64
    make_wheel "$1" pulsehive "$ver" cp311 abi3 manylinux_2_28_x86_64
    make_wheel "$1" pulsehive "$ver" cp311 abi3 win_amd64
  }

  expect_ok() { # <label> <dir> <expect-version>
    local out rc
    out=$(check_dist "$2" "$3" 2>&1)
    rc=$?
    if [ "$rc" -ne 0 ]; then
      echo "self-test: FAILED [$1]: expected publishable, got rc=$rc: $out" >&2
      exit 1
    fi
  }
  expect_reject() { # <label> <dir> <expect-version> <named-error substring>
    local out rc
    out=$(check_dist "$2" "$3" 2>&1)
    rc=$?
    if [ "$rc" -eq 0 ]; then
      echo "self-test: FAILED [$1]: expected rejection, but the check exited 0" >&2
      exit 1
    fi
    case "$out" in
      *"$4"*) : ;;
      *)
        echo "self-test: FAILED [$1]: rejected (rc=$rc) but without the named error ('$4'): $out" >&2
        exit 1
        ;;
    esac
  }

  # 1. A correctly-formed, unpublished set passes (stub: the per-version
  #    endpoint answers 404, which is also what a project that has never been
  #    published answers — hence the check below that the probe really asked
  #    for the endpoint it claims to).
  SELFTEST_HTTP_CODE="404"
  d="$tmp/pass"
  make_set "$d"
  expect_ok "correctly-formed set (first-ever publish)" "$d" "$ver"
  grep -qxF "$PYPI_JSON_URL/pulsehive/$ver/json" "$SELFTEST_HTTP_LOG" || {
    echo "self-test: FAILED [probe endpoint]: the gate never asked the per-version endpoint for '$ver'" >&2
    exit 1
  }

  # 2. An advertised target's wheel missing from the set.
  d="$tmp/missing"
  make_set "$d"
  rm "$d/pulsehive-$ver-cp311-abi3-win_amd64.whl"
  expect_reject "missing target" "$d" "$ver" \
    "missing wheel for advertised target 'x86_64-pc-windows-msvc'"

  # 3. A wheel whose platform tag does not match any advertised target.
  d="$tmp/mislabelled"
  make_set "$d"
  mv "$d/pulsehive-$ver-cp311-abi3-macosx_11_0_arm64.whl" \
    "$d/pulsehive-$ver-cp311-abi3-sunos_x86.whl"
  expect_reject "mislabelled target" "$d" "$ver" \
    "platform tag 'sunos_x86' does not match any advertised target"

  # 4. A wheel whose version disagrees with --expect-version.
  d="$tmp/version"
  make_set "$d"
  mv "$d/pulsehive-$ver-cp311-abi3-win_amd64.whl" \
    "$d/pulsehive-0.3.0b2-cp311-abi3-win_amd64.whl"
  expect_reject "version disagreement" "$d" "$ver" \
    "carries version '0.3.0b2' but --expect-version is '$ver'"

  # 5. The version already exists on PyPI (stub: the per-version endpoint
  # answers 200) — a hard failure, decided by the status and never by the
  # shape of a JSON body.
  SELFTEST_HTTP_CODE="200"
  d="$tmp/published"
  make_set "$d"
  expect_reject "already published" "$d" "$ver" "already published"

  # 5b. ...and a probe that cannot establish the state (stub: HTTP 503)
  # refuses to publish rather than assuming the version is free.
  SELFTEST_HTTP_CODE="503"
  expect_reject "unverifiable PyPI state" "$d" "$ver" "cannot verify PyPI state"

  # 6. A prerelease candidate set: the wheels carry maturin's spelling
  # (`3.0.0b1`) while --expect-version arrives as the tag's Cargo spelling
  # (`3.0.0-beta.1`). Those name the same version, so the set is publishable.
  SELFTEST_HTTP_CODE="404"
  d="$tmp/prerelease"
  mkdir -p "$d"
  make_wheel "$d" pulsehive "$wheel_pre" cp311 abi3 macosx_11_0_arm64
  make_wheel "$d" pulsehive "$wheel_pre" cp311 abi3 manylinux_2_28_x86_64
  make_wheel "$d" pulsehive "$wheel_pre" cp311 abi3 win_amd64
  expect_ok "prerelease set against the tag's Cargo spelling" "$d" "$cargo_pre"

  # 7. ...and the normalization must not turn a different prerelease into the
  # same version: `3.0.0b1` wheels are not `3.0.0-beta.2`.
  expect_reject "prerelease set against a different prerelease" "$d" "3.0.0-beta.2" \
    "carries version '$wheel_pre' but --expect-version is '3.0.0-beta.2'"

  # 8. Wheels only: any other file in dist/ is a named rejection rather than
  # something the uploader has to notice — including a dotfile, which is
  # exactly the kind of stray a macOS build agent leaves behind.
  SELFTEST_HTTP_CODE="404"
  d="$tmp/stray"
  make_set "$d"
  : > "$d/pulsehive-$ver.tar.gz"
  expect_reject "stray tarball in dist" "$d" "$ver" "non-wheel artifact(s) in '$d': pulsehive-$ver.tar.gz"

  d="$tmp/stray-hidden"
  make_set "$d"
  : > "$d/.DS_Store"
  expect_reject "stray dotfile in dist" "$d" "$ver" "non-wheel artifact(s) in '$d': .DS_Store"

  echo "self-test: ok"
}

main() {
  local dist_dir="" expect_version="" self_test=0
  while [ $# -gt 0 ]; do
    case "$1" in
      --dist)
        [ $# -ge 2 ] || {
          usage >&2
          exit 2
        }
        dist_dir="$2"
        shift 2
        ;;
      --expect-version)
        [ $# -ge 2 ] || {
          usage >&2
          exit 2
        }
        expect_version="$2"
        shift 2
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

  if [ -z "$dist_dir" ] || [ -z "$expect_version" ]; then
    usage >&2
    echo "$PROG: either --self-test, or both --dist <dir> and --expect-version <version>, is required" >&2
    exit 2
  fi

  check_dist "$dist_dir" "$expect_version"
}

SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
[ -r "$SCRIPT_DIR/lib/pep440.sh" ] ||
  fail "missing the version rule $SCRIPT_DIR/lib/pep440.sh (it ships next to this script)"
# shellcheck source=lib/pep440.sh
. "$SCRIPT_DIR/lib/pep440.sh"

main "$@"
