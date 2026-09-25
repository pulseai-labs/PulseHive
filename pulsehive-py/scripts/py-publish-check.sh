#!/usr/bin/env bash
# py-publish-check.sh — fail-closed pre-publish gate for the `pulsehive` Python
# distribution (r2.s2.w2). RELEASE.md's negative control for the release path;
# demo line d24.
#
# Modes:
#   --dist <dir> --expect-version <version>
#       Verify that <dir> holds a complete, correctly-labelled, correctly-
#       versioned wheel set covering every advertised target, and that
#       <version> is NOT already published on PyPI (JSON API query; an
#       already-published version is a hard failure, never a warning). Every
#       rejection exits non-zero with a named error on stderr; a publishable
#       set prints a one-line summary and exits 0.
#   --self-test
#       Hermetic (no network, no credentials): builds throwaway candidate sets
#       under a temp dir and asserts that each of the four rejections fires
#       AND that a correctly-formed set passes. Prints `self-test: ok` as its
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

# pypi_fetch_project_json <project>
#   Prints the project's JSON API body; non-zero when the query itself fails.
#   Split from the version match below so --self-test can stub only the
#   network and exercise the real parsing.
pypi_fetch_project_json() {
  curl -fsSL --max-time 30 "$PYPI_JSON_URL/$1/json" 2>/dev/null
}

# pypi_has_version <project> <version>
#   rc 0 — <version> is published for <project>
#   rc 1 — <version> is not published
#   rc 2 — the query itself failed (network/API); callers must fail closed
pypi_has_version() {
  local project="$1" version="$2" json esc
  esc=$(printf '%s' "$version" | sed -e 's/[.[]/\\&/g')
  json=$(pypi_fetch_project_json "$project") || return 2
  # The JSON API maps every released version to an ARRAY of files
  # ("1.2.3": [ ... ]) — never to a bare object. Matching an object shape
  # here silently reports published versions as unpublished.
  printf '%s' "$json" | grep -qE "\"${esc}\":[[:space:]]*\["
}

check_dist() {
  local dist_dir="$1" expect_version="$2"
  [ -d "$dist_dir" ] || fail "dist directory '$dist_dir' not found"

  local wheels=()
  shopt -s nullglob
  wheels=("$dist_dir"/*.whl)
  shopt -u nullglob
  [ "${#wheels[@]}" -gt 0 ] || fail "no wheels (*.whl) found in '$dist_dir'"

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
    [ "$version" = "$expect_version" ] ||
      fail "version disagreement: wheel '$base' carries version '$version' but --expect-version is '$expect_version'"
  done

  for target in "${TARGETS[@]}"; do
    case "$seen" in
      *" $target "*) : ;;
      *) fail "missing wheel for advertised target '$target' (looked in '$dist_dir')" ;;
    esac
  done

  local rc=0
  pypi_has_version "$name" "$expect_version" || rc=$?
  case $rc in
    0) fail "already published: version '$expect_version' of project '$name' already exists on PyPI — re-publishing an existing version is forbidden" ;;
    1) : ;;
    *) fail "cannot verify PyPI state for '$name' (query failed) — refusing to publish an unproven version" ;;
  esac

  echo "$PROG: ok: ${#wheels[@]} wheel(s) cover all ${#TARGETS[@]} advertised targets at version '$expect_version' (project '$name') — publishable"
}

self_test() {
  # tmp is deliberately global so the EXIT trap below can still see it
  tmp=""
  tmp=$(mktemp -d "${TMPDIR:-/tmp}/py-publish-check-selftest.XXXXXX") ||
    fail "self-test: cannot create a temp dir"
  trap 'rm -rf "${tmp:-}"' EXIT

  # Hermetic: stub ONLY the network fetch, feeding canned PyPI JSON bodies —
  # the version-matching parse inside pypi_has_version runs for real, so a
  # wrong releases-shape assumption fails here offline instead of on PyPI.
  # The bodies mirror the live API: every key under "releases" maps to an
  # ARRAY of files. The unpublished body carries the candidate version only
  # as info.version (a value), never as a releases key.
  pypi_fetch_project_json() { printf '%s' "$SELFTEST_PYPI_JSON"; }
  SELFTEST_PYPI_JSON=""
  local canned_published canned_unpublished
  canned_published='{
 "info": {"name": "pulsehive", "version": "3.0.0"},
 "releases": {
  "2.0.0rc1": [{"filename": "pulsehive-2.0.0rc1-cp39-abi3-win_amd64.whl"}],
  "3.0.0": [{"filename": "pulsehive-3.0.0-cp311-abi3-macosx_11_0_arm64.whl"}]
 },
 "urls": []
}'
  canned_unpublished='{
 "info": {"name": "pulsehive", "version": "3.0.0"},
 "releases": {
  "2.0.0rc1": [{"filename": "pulsehive-2.0.0rc1-cp39-abi3-win_amd64.whl"}]
 },
 "urls": []
}'

  local ver="3.0.0"

  # 0. The parse that decides "already published" judges the canned bodies.
  SELFTEST_PYPI_JSON="$canned_published"
  if ! pypi_has_version pulsehive "$ver"; then
    echo "self-test: FAILED [pypi parse]: published '$ver' not detected in an array-shaped releases body" >&2
    exit 1
  fi
  SELFTEST_PYPI_JSON="$canned_unpublished"
  if pypi_has_version pulsehive "$ver"; then
    echo "self-test: FAILED [pypi parse]: a version present only as a value was treated as a published release" >&2
    exit 1
  fi

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

  # 1. A correctly-formed, unpublished set passes (canned body: candidate
  #    version absent from "releases").
  SELFTEST_PYPI_JSON="$canned_unpublished"
  d="$tmp/pass"
  make_set "$d"
  expect_ok "correctly-formed set" "$d" "$ver"

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

  # 5. The version already exists on PyPI — decided by the real parse against
  # a canned array-shaped releases body (never a real query).
  SELFTEST_PYPI_JSON="$canned_published"
  d="$tmp/published"
  make_set "$d"
  expect_reject "already published" "$d" "$ver" "already published"

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

main "$@"
