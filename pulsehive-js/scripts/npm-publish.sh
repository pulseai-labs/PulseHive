#!/usr/bin/env bash
#
# npm-publish.sh — the single gated npm publish path for @pulsehive/sdk.
#
# The publish job of .github/workflows/npm-release.yml runs exactly one
# invocation of this script; a developer runs the same commands by hand. The
# tarballs it publishes are the bytes `verify-install` installed and executed
# (RELEASE.md, artifact identity), so nothing here re-packs and nothing is
# built.
#
# Usage
#   npm-publish.sh probe <name> <version>
#       read-only registry lookup printing
#       `<name>@<version>: not published` or `<name>@<version>: published <integrity>`.
#   npm-publish.sh plan --dist <dir> --expect-version <v>
#       validate the tarball set, print every tarball's decision, then
#       `npm-publish: plan ok`.
#   npm-publish.sh publish --dist <dir> --expect-version <v> [--fresh-build]
#       validate, decide, publish (platform packages first, the main package
#       last), re-read each published version's `dist.integrity` and assert it
#       is the tested tarball's: `npm-publish: published <n>, skipped <m>`.
#       `--fresh-build` appends L4's remedy to a different-bytes refusal (the
#       workflow passes it on workflow_dispatch, where the run is a fresh build
#       rather than a resume of the original run).
#   npm-publish.sh --self-test
#       hermetic proof of the skip-or-fail table below (#100): a stub `npm`
#       first on PATH in a temp dir, throwaway tarballs, no network.
#       Prints one `case <label>: <ok|rejected>` line each and `self-test: ok`.
#
# Set validation. <dir> holds exactly one main tarball and one per advertised
# suffix (the three targets package.json advertises through napi.targets) and
# nothing else. The main package's name is `release-check.sh --get npm.package`,
# each platform package's is `<main>-<suffix>`, every tarball's
# `package/package.json` version is <v>, and every file name is the one `npm
# pack` gives that package.
#
# Decision per tarball — npm's own published integrity is the identity check:
#   E404                        -> publish
#   integrity == local tarball  -> skip (already published with the tested bytes)
#   integrity != local tarball  -> refuse, publish nothing further (L4: a
#                                  version whose bytes differ is not this tag's)
#   any other registry failure  -> refuse
# The main package is never published unless every platform package is
# published or skipped.
#
# An empty NODE_AUTH_TOKEN refuses before any registry call (A4): a missing
# credential fails the job, it never passes it with a warning.
#
# Every registry call goes through one `npm` invocation (`registry_view` /
# `registry_publish`); that pair is the seam `--self-test` replaces.
#
set -uo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
PKG_DIR="$(cd -- "$SCRIPT_DIR/.." && pwd -P)"
ROOT="$(cd -- "$PKG_DIR/.." && pwd -P)"
PKG_JSON="$PKG_DIR/package.json"
RELEASE_CHECK="$ROOT/scripts/release-check.sh"

PROG="npm-publish"

say() { printf '%s\n' "$*"; }
error_line() { printf '%s: ERROR: %s\n' "$PROG" "$*" >&2; }

usage() {
  awk '/^# Usage$/{show=1} show && /^set -uo pipefail$/{exit} show{sub(/^# ?/,""); print}' "${BASH_SOURCE[0]}"
}

# --------------------------------------------------------------- manifest ----

# Read a dotted path out of package.json. Arrays come back newline-separated.
pkg_get() {
  node -e '
    const fs = require("fs");
    const [file, path] = process.argv.slice(1);
    let v = JSON.parse(fs.readFileSync(file, "utf8"));
    for (const key of path.split(".")) {
      v = (v === null || v === undefined) ? undefined : v[key];
    }
    if (v === null || v === undefined) process.exit(0);
    process.stdout.write(Array.isArray(v) ? v.join("\n") : String(v));
  ' "$PKG_JSON" "$1"
}

# The advertised matrix (ADR-015 A1): every Rust target package.json declares
# must be one of the three this package publishes. An unknown target is a hard
# error — guessing would mislabel a package — and a target set that has grown
# past this table fails closed until the table and pack-npm.sh agree again.
platform_for_target() {
  case "$1" in
    aarch64-apple-darwin) echo "darwin-arm64" ;;
    x86_64-unknown-linux-gnu) echo "linux-x64-gnu" ;;
    x86_64-pc-windows-msvc) echo "win32-x64-msvc" ;;
    *) return 1 ;;
  esac
}

MAIN_NAME=""
ADVERTISED_SUFFIXES=()
load_advertised() {
  MAIN_NAME="$(bash "$RELEASE_CHECK" --get npm.package)" || {
    error_line "cannot read npm.package from the release manifest"
    return 1
  }
  [ -n "$MAIN_NAME" ] || {
    error_line "release-check.sh --get npm.package printed nothing"
    return 1
  }

  local declared suffix t
  declared="$(pkg_get napi.targets)"
  [ -n "$declared" ] || {
    error_line "package.json declares no napi.targets"
    return 1
  }
  ADVERTISED_SUFFIXES=()
  while IFS= read -r t; do
    [ -n "$t" ] || continue
    if ! suffix="$(platform_for_target "$t")"; then
      error_line "napi target '$t' has no npm platform package name"
      return 1
    fi
    ADVERTISED_SUFFIXES+=("$suffix")
  done <<<"$declared"

  local known
  for suffix in "${ADVERTISED_SUFFIXES[@]}"; do
    known=0
    for t in "${ADVERTISED_SUFFIXES[@]}"; do
      [ "$t" = "$suffix" ] && known=1 && break
    done
    [ "$known" -eq 1 ] || { error_line "advertised suffix '$suffix' is not in the matrix"; return 1; }
  done
  return 0
}

# `npm pack`'s file name for a package: the name without its scope, its slashes
# as dashes, then `-<version>.tgz` (@pulsehive/sdk -> pulsehive-sdk-3.0.0.tgz).
tarball_name_for() {
  local name="${1#@}"
  printf '%s-%s.tgz\n' "${name//\//-}" "$2"
}

# ---------------------------------------------------------------- tarballs ---

tarball_package_json() { # <tarball>
  tar -xzOf "$1" package/package.json 2>/dev/null
}

tarball_meta() { # <tarball> ; prints "<name>\t<version>"
  local out
  out="$(tarball_package_json "$1" | node -e '
    let raw = "";
    process.stdin.on("data", (chunk) => (raw += chunk));
    process.stdin.on("end", () => {
      const data = JSON.parse(raw);
      if (typeof data.name !== "string" || typeof data.version !== "string") process.exit(1);
      process.stdout.write(data.name + "\t" + data.version);
    });
  ')" || return 1
  printf '%s' "$out"
}

tarball_integrity() { # <tarball> ; prints "sha512-<base64>" over the file bytes
  node -e '
    const crypto = require("crypto");
    const fs = require("fs");
    process.stdout.write(
      "sha512-" + crypto.createHash("sha512").update(fs.readFileSync(process.argv[1])).digest("base64"),
    );
  ' "$1"
}

declare -A SUFFIX_FILE=()
MAIN_FILE=""

# One tarball's problems, printed as they are found. Sets ENTRY_KIND to
# main | platform:<suffix> | foreign and ENTRY_PROBLEMS to 1 when anything was
# printed; validate_dist reads both immediately after the call. Not a command
# substitution: that runs the helper in a subshell, where neither variable could
# reach the caller.
_classify_dist_tarball() { # <dir> <base> <want>
  local dir="$1" base="$2" want="$3" meta name version expected_file suffix
  ENTRY_KIND=""
  ENTRY_PROBLEMS=0
  if ! meta="$(tarball_meta "$dir/$base")"; then
    printf 'npm-publish: ERROR: cannot read package/package.json from %s\n' "$base"
    ENTRY_PROBLEMS=1
    return 0
  fi

  name="${meta%%$'\t'*}"
  version="${meta#*$'\t'}"
  expected_file="$(tarball_name_for "$name" "$version")"
  if [ "$base" != "$expected_file" ]; then
    printf 'npm-publish: ERROR: %s holds %s@%s, which npm pack would name %s\n' \
      "$base" "$name" "$version" "$expected_file"
    ENTRY_PROBLEMS=1
  fi
  if [ "$version" != "$want" ]; then
    printf 'npm-publish: ERROR: %s holds %s@%s; expected version %s\n' "$base" "$name" "$version" "$want"
    ENTRY_PROBLEMS=1
  fi

  if [ "$name" = "$MAIN_NAME" ]; then
    ENTRY_KIND="main"
    return 0
  fi
  for suffix in "${ADVERTISED_SUFFIXES[@]}"; do
    if [ "$name" = "$MAIN_NAME-$suffix" ]; then
      ENTRY_KIND="platform:$suffix"
      return 0
    fi
  done
  printf 'npm-publish: ERROR: unexpected package name %s in %s (expected %s or %s-<suffix>)\n' \
    "$name" "$base" "$MAIN_NAME" "$MAIN_NAME"
  ENTRY_PROBLEMS=1
  ENTRY_KIND="foreign"
  return 0
}

# Prints one problem per line; rc 1 when the directory is not exactly one main
# tarball plus one per advertised suffix, and nothing else.
validate_dist() { # <dir> <expect-version>
  local dir="$1" want="$2" rc=0 entry suffix kind
  if [ ! -d "$dir" ]; then
    printf 'npm-publish: ERROR: directory %s does not exist\n' "$dir"
    return 1
  fi

  local -a entries=()
  while IFS= read -r entry; do
    [ -n "$entry" ] || continue
    entries+=("$entry")
  done < <(cd -- "$dir" && ls -A 2>/dev/null | LC_ALL=C sort)

  for entry in "${entries[@]}"; do
    case "$entry" in
      *.tgz) ;;
      *) printf 'npm-publish: ERROR: unexpected entry %s in %s (only release tarballs belong there)\n' \
        "$entry" "$dir"
        rc=1 ;;
    esac
  done

  SUFFIX_FILE=()
  MAIN_FILE=""
  for entry in "${entries[@]}"; do
    case "$entry" in *.tgz) ;; *) continue ;; esac
    _classify_dist_tarball "$dir" "$entry" "$want"
    [ "$ENTRY_PROBLEMS" -eq 0 ] || rc=1
    # Bookkeeping only: an unreadable or foreign entry has already been counted.
    case "$ENTRY_KIND" in
      main)
        if [ -n "$MAIN_FILE" ]; then
          printf 'npm-publish: ERROR: more than one main %s tarball (%s and %s)\n' "$MAIN_NAME" "$MAIN_FILE" "$entry"
          rc=1
        else
          MAIN_FILE="$entry"
        fi
        ;;
      platform:*)
        kind="${ENTRY_KIND#platform:}"
        if [ -n "${SUFFIX_FILE[$kind]:-}" ]; then
          printf 'npm-publish: ERROR: more than one %s tarball (%s and %s)\n' \
            "$MAIN_NAME-$kind" "${SUFFIX_FILE[$kind]}" "$entry"
          rc=1
        else
          SUFFIX_FILE["$kind"]="$entry"
        fi
        ;;
    esac
  done

  if [ -n "$MAIN_FILE" ]; then :; else
    printf 'npm-publish: ERROR: no main %s tarball in %s\n' "$MAIN_NAME" "$dir"
    rc=1
  fi
  for suffix in "${ADVERTISED_SUFFIXES[@]}"; do
    if [ -z "${SUFFIX_FILE[$suffix]:-}" ]; then
      printf 'npm-publish: ERROR: missing platform tarball for suffix %s (%s-%s)\n' "$suffix" "$MAIN_NAME" "$suffix"
      rc=1
    fi
  done
  return "$rc"
}

# ---------------------------------------------------------------- registry ---

# The only read path to npm. Prints one of:
#   INTEGRITY:<value>   the published integrity of <name>@<version>
#   E404                the version, or the package, is not on the registry
#   ERROR:<detail>      any other registry failure
registry_view() { # <name> <version>
  local spec="$1@$2" out rc
  if out="$(npm view "$spec" dist.integrity 2>&1)"; then rc=0; else rc=$?; fi
  if [ "$rc" -eq 0 ]; then
    out="$(printf '%s\n' "$out" | tr -d '\r' | awk 'NF { line = $0 } END { print line }')"
    if [ -n "$out" ]; then
      printf 'INTEGRITY:%s\n' "$out"
      return 0
    fi
    printf 'ERROR:npm view %s printed no integrity\n' "$spec"
    return 0
  fi
  if printf '%s' "$out" | grep -q 'E404'; then
    printf 'E404\n'
    return 0
  fi
  printf 'ERROR:%s\n' "$(printf '%s' "$out" | tr '\n' ' ' | cut -c1-240)"
  return 0
}

# The only write path to npm.
registry_publish() { # <tarball>
  npm publish --access public "$1"
}

# --------------------------------------------------------------- decisions ---

DECISION=""

# Prints the decision line; sets DECISION to publish|skip|refuse. The token
# check is the caller's: plan and probe never read the registry with a token.
decide_one() { # <name> <version> <local-integrity> <fresh-build: 0|1>
  local name="$1" version="$2" local_integrity="$3" fresh="$4" probe hint=""
  probe="$(registry_view "$name" "$version")"
  case "$probe" in
    E404)
      say "npm-publish: $name@$version: publishing"
      DECISION=publish
      ;;
    "INTEGRITY:$local_integrity")
      say "npm-publish: $name@$version already published with the tested bytes — skipped"
      DECISION=skip
      ;;
    INTEGRITY:*)
      if [ "$fresh" -eq 1 ]; then
        hint=" — this is a fresh build; re-run the failed jobs of the original tag run instead"
      fi
      error_line "$name@$version is already on npm with different bytes (registry ${probe#INTEGRITY:}, tested $local_integrity) — refusing$hint"
      DECISION=refuse
      return 1
      ;;
    *)
      error_line "cannot read npm for $name@$version (${probe#ERROR:}) — refusing"
      DECISION=refuse
      return 1
      ;;
  esac
  return 0
}

PLAN_FILES=()
PLAN_NAMES=()
PLAN_VERSIONS=()
PLAN_INTEGRITY=()
PLAN_DECISION=()

# Decides every tarball, platform packages first and the main package last.
# Prints one decision line per tarball; rc 1 on the first refusal.
build_plan() { # <dist> <expect-version> <fresh-build: 0|1>
  local dir="$1" want="$2" fresh="$3" suffix i integrity
  PLAN_FILES=()
  PLAN_NAMES=()
  PLAN_VERSIONS=()
  PLAN_INTEGRITY=()
  PLAN_DECISION=()

  for suffix in "${ADVERTISED_SUFFIXES[@]}"; do
    PLAN_FILES+=("$dir/${SUFFIX_FILE[$suffix]}")
    PLAN_NAMES+=("$MAIN_NAME-$suffix")
    PLAN_VERSIONS+=("$want")
  done
  PLAN_FILES+=("$dir/$MAIN_FILE")
  PLAN_NAMES+=("$MAIN_NAME")
  PLAN_VERSIONS+=("$want")

  for i in "${!PLAN_FILES[@]}"; do
    integrity="$(tarball_integrity "${PLAN_FILES[$i]}")"
    PLAN_INTEGRITY+=("$integrity")
    if ! decide_one "${PLAN_NAMES[$i]}" "${PLAN_VERSIONS[$i]}" "$integrity" "$fresh"; then
      return 1
    fi
    PLAN_DECISION+=("$DECISION")
  done
  return 0
}

# ------------------------------------------------------------------ publish --

# Publishes one tarball and proves the registry's bytes are the tested bytes:
# the published integrity is re-read with bounded retries, because a just-
# published version takes a moment to propagate.
publish_one() { # <index>
  local name="${PLAN_NAMES[$1]}" version="${PLAN_VERSIONS[$1]}"
  local file="${PLAN_FILES[$1]}" local_integrity="${PLAN_INTEGRITY[$1]}"
  local out rc state attempt=1
  local max="${NPM_PUBLISH_RETRY_LIMIT:-5}"
  local delay="${NPM_PUBLISH_RETRY_DELAY:-2}"

  if out="$(registry_publish "$file" 2>&1)"; then rc=0; else rc=$?; fi
  if [ "$rc" -ne 0 ]; then
    printf '%s\n' "$out" >&2
    error_line "npm publish failed for $name@$version (exit $rc) — refusing"
    return 1
  fi

  while :; do
    state="$(registry_view "$name" "$version")"
    [ "$state" = "INTEGRITY:$local_integrity" ] && return 0
    if [ "$attempt" -ge "$max" ]; then
      break
    fi
    attempt=$((attempt + 1))
    sleep "$delay"
  done

  case "$state" in
    E404) state="nothing (E404) after $attempt reads" ;;
    ERROR:*) state="${state#ERROR:}" ;;
    INTEGRITY:*) state="${state#INTEGRITY:}" ;;
  esac
  error_line "$name@$version was published but npm reports $state, not the tested $local_integrity — refusing"
  return 1
}

# ------------------------------------------------------------- entry points ---

parse_dist_args() { # <args...> ; sets DIST_DIR, EXPECT_VERSION, FRESH_BUILD
  DIST_DIR=""
  EXPECT_VERSION=""
  FRESH_BUILD=0
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --dist)
        [ "$#" -ge 2 ] || { error_line "--dist needs a directory"; return 2; }
        DIST_DIR="$2"
        shift 2
        ;;
      --expect-version)
        [ "$#" -ge 2 ] || { error_line "--expect-version needs a version"; return 2; }
        EXPECT_VERSION="$2"
        shift 2
        ;;
      --fresh-build)
        FRESH_BUILD=1
        shift
        ;;
      *)
        error_line "unknown argument '$1'"
        return 2
        ;;
    esac
  done
  [ -n "$DIST_DIR" ] || { error_line "--dist <dir> is required"; return 2; }
  [ -n "$EXPECT_VERSION" ] || { error_line "--expect-version <v> is required"; return 2; }
  return 0
}

cmd_probe() { # <name> <version>
  [ "$#" -eq 2 ] || { error_line "probe takes <name> <version>"; return 2; }
  local name="$1" version="$2" probe
  probe="$(registry_view "$name" "$version")"
  case "$probe" in
    E404)
      say "$name@$version: not published"
      ;;
    INTEGRITY:*)
      say "$name@$version: published ${probe#INTEGRITY:}"
      ;;
    *)
      error_line "cannot read npm for $name@$version (${probe#ERROR:}) — refusing"
      return 1
      ;;
  esac
  return 0
}

cmd_plan() { # --dist <dir> --expect-version <v>
  parse_dist_args "$@" || return $?
  load_advertised || return 1
  validate_dist "$DIST_DIR" "$EXPECT_VERSION" || return 1
  build_plan "$DIST_DIR" "$EXPECT_VERSION" 0 || return 1
  say "npm-publish: plan ok"
  return 0
}

cmd_publish() { # --dist <dir> --expect-version <v> [--fresh-build]
  parse_dist_args "$@" || return $?
  [ -n "${NODE_AUTH_TOKEN:-}" ] || {
    error_line "no npm token in the npm environment — refusing"
    return 1
  }
  load_advertised || return 1
  validate_dist "$DIST_DIR" "$EXPECT_VERSION" || return 1
  build_plan "$DIST_DIR" "$EXPECT_VERSION" "$FRESH_BUILD" || return 1

  local i published=0 skipped=0
  for i in "${!PLAN_FILES[@]}"; do
    case "${PLAN_DECISION[$i]}" in
      publish)
        publish_one "$i" || return 1
        published=$((published + 1))
        ;;
      skip)
        skipped=$((skipped + 1))
        ;;
    esac
  done
  say "npm-publish: published $published, skipped $skipped"
  return 0
}

# ---------------------------------------------------------------- self-test ---
#
# A stub `npm` first on PATH is the whole seam: it serves `view <spec>
# dist.integrity` from a throwaway registry and records `publish` calls there,
# so every case drives the real decision and publish paths with no network.
# The fixtures are throwaway tarballs built from minimal package.json trees —
# nothing here reads or writes the real package.

SELFTEST_TMP=""
SELFTEST_BIN=""
SELFTEST_VERSION="3.0.0"

selftest_cleanup() {
  [ -n "$SELFTEST_TMP" ] || return 0
  rm -rf -- "$SELFTEST_TMP"
  SELFTEST_TMP=""
}

_selftest_fail() {
  printf 'self-test: FAIL — %s\n' "$1"
}

# One `case <label>: <ok|rejected>` line; the extra reason lines are what a
# failing self-test needs to be actionable.
_selftest_case() { # <label> <ok|rejected> <output> <rc>
  local label="$1" expected="$2" out="$3" rc="$4"
  if [ "$expected" = "ok" ]; then
    if [ "$rc" -ne 0 ]; then
      _selftest_fail "$label: expected success, got rc $rc (output: $out)"
      return 1
    fi
  else
    if [ "$rc" -eq 0 ]; then
      _selftest_fail "$label: expected a refusal, but the run succeeded (output: $out)"
      return 1
    fi
  fi
  say "case $label: $expected"
  return 0
}

_selftest_has() { # <label> <needle> <haystack>
  if ! printf '%s' "$3" | grep -qF -- "$2"; then
    _selftest_fail "$1: output does not contain '$2' (got: $3)"
    return 1
  fi
  return 0
}

_selftest_lacks() { # <label> <needle> <haystack>
  if printf '%s' "$3" | grep -qF -- "$2"; then
    _selftest_fail "$1: output must not contain '$2' (got: $3)"
    return 1
  fi
  return 0
}

_selftest_tarball() { # <dir> <package-name> <version>
  local dir="$1" name="$2" version="$3"
  local base tree
  base="$(tarball_name_for "$name" "$version")"
  tree="$(mktemp -d "$SELFTEST_TMP/tree.XXXXXX")"
  mkdir -p "$tree/package"
  printf '{"name":"%s","version":"%s"}\n' "$name" "$version" >"$tree/package/package.json"
  tar -czf "$dir/$base" -C "$tree" package/package.json
}

# The standard valid set: the main package plus every advertised suffix.
_selftest_dist() { # <dir>
  local dir="$1" suffix
  mkdir -p "$dir"
  _selftest_tarball "$dir" "$MAIN_NAME" "$SELFTEST_VERSION"
  for suffix in "${ADVERTISED_SUFFIXES[@]}"; do
    _selftest_tarball "$dir" "$MAIN_NAME-$suffix" "$SELFTEST_VERSION"
  done
}

_selftest_state() { # <name> ; prints a fresh stub-state directory
  local state="$SELFTEST_TMP/state-$1"
  mkdir -p "$state/registry"
  : >"$state/calls.log"
  printf '%s' "$state"
}

# Registers a tarball in the stub registry without counting it as a case call
# (seeding "this version is already published with these bytes").
_selftest_seed() { # <state> <tarball>
  ( PATH="$SELFTEST_BIN:$PATH" STUB_STATE="$1" npm publish --access public "$2" ) >/dev/null 2>&1
  : >"$1/calls.log"
}

_selftest_publishes() { # <state> ; prints the `publish <spec>` lines so far
  grep '^publish ' "$1/calls.log" 2>/dev/null || true
}

# Runs `cmd_publish` in a subshell with the stub on PATH. A token is set unless
# the case wants the missing-credential refusal.
_selftest_publish() { # <state> <dist> <with-token> [extra args...]
  local state="$1" dist="$2" with_token="$3"
  shift 3
  (
    export PATH="$SELFTEST_BIN:$PATH"
    export STUB_STATE="$state"
    NPM_PUBLISH_RETRY_DELAY=0
    if [ "$with_token" = "1" ]; then
      NODE_AUTH_TOKEN="stub-token"
    else
      unset NODE_AUTH_TOKEN
    fi
    cmd_publish --dist "$dist" --expect-version "$SELFTEST_VERSION" "$@"
  ) 2>&1
}

# A foreign-name tarball carries a package name that is neither the main
# package nor one of its platform packages.
_selftest_foreign_tarball() { # <dir>
  _selftest_tarball "$1" "@pulsehive/not-this-sdk" "$SELFTEST_VERSION"
}

write_stub_npm() { # <path>
  cat >"$1" <<'STUB'
#!/usr/bin/env node
'use strict';
// Stub `npm` for npm-publish.sh --self-test. Serves `view <spec>
// dist.integrity` from a throwaway registry under STUB_STATE and records every
// `publish` there. No network, no real registry.
const fs = require('fs');
const path = require('path');
const crypto = require('crypto');
const { execFileSync } = require('child_process');

const state = process.env.STUB_STATE;
if (!state) {
  process.stderr.write('stub npm: STUB_STATE is not set\n');
  process.exit(2);
}
const registry = path.join(state, 'registry');
fs.mkdirSync(registry, { recursive: true });

const record = (line) => fs.appendFileSync(path.join(state, 'calls.log'), line + '\n');
const statePath = (spec) => path.join(registry, encodeURIComponent(spec));
const integrityOf = (file) =>
  'sha512-' + crypto.createHash('sha512').update(fs.readFileSync(file)).digest('base64');
const tarballMeta = (tarball) =>
  JSON.parse(execFileSync('tar', ['-xzOf', tarball, 'package/package.json'], { encoding: 'utf8' }));

const argv = process.argv.slice(2);

if (argv[0] === 'view') {
  const spec = argv[1];
  record('view ' + spec);
  if (process.env.STUB_VIEW_ERROR_FOR === spec) {
    process.stderr.write('npm ERR! code E500\nnpm ERR! 500 Internal Server Error\n');
    process.exit(1);
  }
  const file = statePath(spec);
  if (!fs.existsSync(file)) {
    process.stderr.write("npm ERR! code E404\nnpm ERR! 404 '" + spec + "' is not in this registry.\n");
    process.exit(1);
  }
  process.stdout.write(fs.readFileSync(file, 'utf8').trim() + '\n');
  process.exit(0);
}

if (argv[0] === 'publish') {
  const tarball = argv[argv.length - 1];
  const meta = tarballMeta(tarball);
  const spec = meta.name + '@' + meta.version;
  record('publish ' + spec);
  let integrity = integrityOf(tarball);
  if (process.env.STUB_LIE_AFTER_PUBLISH_FOR === spec) {
    integrity = 'sha512-' + Buffer.from('not the tested bytes').toString('base64');
  }
  fs.writeFileSync(statePath(spec), integrity + '\n');
  process.stdout.write('+ ' + spec + '\n');
  process.exit(0);
}

process.stderr.write('stub npm: unsupported invocation: ' + argv.join(' ') + '\n');
process.exit(2);
STUB
  chmod +x "$1"
}

run_self_test() {
  local rc=0 tmp state dist out
  tmp="$(mktemp -d)"
  SELFTEST_TMP="$tmp"
  trap selftest_cleanup EXIT
  SELFTEST_BIN="$tmp/bin"
  mkdir -p "$SELFTEST_BIN"
  write_stub_npm "$SELFTEST_BIN/npm"

  load_advertised || return 1

  local platform suffix name spec
  local -a platforms=()
  for suffix in "${ADVERTISED_SUFFIXES[@]}"; do
    platforms+=("$suffix")
  done

  # 1. first-publish: nothing published yet — every tarball is published, the
  #    platform packages first and the main package last.
  state="$(_selftest_state first-publish)"
  dist="$tmp/dist-first-publish"
  _selftest_dist "$dist"
  out="$(_selftest_publish "$state" "$dist" 1)"
  if ! _selftest_case "first-publish" ok "$out" "$?"; then rc=1; fi
  _selftest_has "first-publish" "npm-publish: published 4, skipped 0" "$out" || rc=1
  _selftest_lacks "first-publish" "ERROR" "$out" || rc=1
  local expected_order="" published_order
  for suffix in "${platforms[@]}"; do
    expected_order+="publish $MAIN_NAME-$suffix@$SELFTEST_VERSION"$'\n'
  done
  expected_order+="publish $MAIN_NAME@$SELFTEST_VERSION"$'\n'
  published_order="$(_selftest_publishes "$state")"
  if [ "$published_order" != "${expected_order%$'\n'}" ]; then
    _selftest_fail "first-publish: published order is '$published_order', expected '${expected_order%$'\n'}'"
    rc=1
  fi

  # 2. resume: two platform packages are already published with the tested
  #    bytes — they are skipped and the rest still publishes (#100).
  state="$(_selftest_state resume)"
  dist="$tmp/dist-resume"
  _selftest_dist "$dist"
  _selftest_seed "$state" "$dist/$(tarball_name_for "$MAIN_NAME-${platforms[1]}" "$SELFTEST_VERSION")"
  _selftest_seed "$state" "$dist/$(tarball_name_for "$MAIN_NAME-${platforms[2]}" "$SELFTEST_VERSION")"
  out="$(_selftest_publish "$state" "$dist" 1)"
  _selftest_case "resume" ok "$out" "$?" || rc=1
  _selftest_has "resume" "npm-publish: published 2, skipped 2" "$out" || rc=1
  published_order="$(_selftest_publishes "$state")"
  expected_order="publish $MAIN_NAME-${platforms[0]}@$SELFTEST_VERSION"$'\n'"publish $MAIN_NAME@$SELFTEST_VERSION"
  if [ "$published_order" != "$expected_order" ]; then
    _selftest_fail "resume: published '$published_order', expected '$expected_order'"
    rc=1
  fi

  # 3. complete: every version is already published with the tested bytes —
  #    nothing is published and the run succeeds.
  state="$(_selftest_state complete)"
  dist="$tmp/dist-complete"
  _selftest_dist "$dist"
  for suffix in "" "${platforms[@]}"; do
    if [ -z "$suffix" ]; then
      name="$MAIN_NAME"
    else
      name="$MAIN_NAME-$suffix"
    fi
    _selftest_seed "$state" "$dist/$(tarball_name_for "$name" "$SELFTEST_VERSION")"
  done
  out="$(_selftest_publish "$state" "$dist" 1)"
  _selftest_case "complete" ok "$out" "$?" || rc=1
  _selftest_has "complete" "npm-publish: published 0, skipped 4" "$out" || rc=1
  if [ -n "$(_selftest_publishes "$state")" ]; then
    _selftest_fail "complete: nothing may be published, but the registry got $( _selftest_publishes "$state" )"
    rc=1
  fi

  # 4. different-bytes: a platform package is on npm with other bytes. The run
  #    refuses and never reaches the main package.
  state="$(_selftest_state different-bytes)"
  dist="$tmp/dist-different-bytes"
  _selftest_dist "$dist"
  local tamper_tree="$tmp/tamper-tree"
  mkdir -p "$tamper_tree/package"
  printf '{"name":"%s","version":"%s","tampered":true}\n' \
    "$MAIN_NAME-${platforms[0]}" "$SELFTEST_VERSION" >"$tamper_tree/package/package.json"
  local tampered_tarball="$tmp/tampered.tgz"
  tar -czf "$tampered_tarball" -C "$tamper_tree" package/package.json
  _selftest_seed "$state" "$tampered_tarball"
  out="$(_selftest_publish "$state" "$dist" 1)"
  _selftest_case "different-bytes" rejected "$out" "$?" || rc=1
  _selftest_has "different-bytes" "is already on npm with different bytes" "$out" || rc=1
  _selftest_lacks "different-bytes" "fresh build; re-run the failed jobs" "$out" || rc=1
  if printf '%s' "$(_selftest_publishes "$state")" | grep -qF -- "publish $MAIN_NAME@"; then
    _selftest_fail "different-bytes: the main package must not be published"
    rc=1
  fi

  # 5. registry-error: a read that fails for any reason other than E404.
  state="$(_selftest_state registry-error)"
  dist="$tmp/dist-registry-error"
  _selftest_dist "$dist"
  export STUB_VIEW_ERROR_FOR="$MAIN_NAME-${platforms[0]}@$SELFTEST_VERSION"
  out="$(_selftest_publish "$state" "$dist" 1)"
  local case_rc=$?
  unset STUB_VIEW_ERROR_FOR
  _selftest_case "registry-error" rejected "$out" "$case_rc" || rc=1
  _selftest_has "registry-error" "cannot read npm for $MAIN_NAME-${platforms[0]}@$SELFTEST_VERSION" "$out" || rc=1

  # 6. no-token: an empty NODE_AUTH_TOKEN refuses before any registry call (A4).
  state="$(_selftest_state no-token)"
  dist="$tmp/dist-no-token"
  _selftest_dist "$dist"
  out="$(_selftest_publish "$state" "$dist" 0)"
  _selftest_case "no-token" rejected "$out" "$?" || rc=1
  _selftest_has "no-token" "no npm token in the npm environment — refusing" "$out" || rc=1
  if [ -s "$state/calls.log" ]; then
    _selftest_fail "no-token: nothing may be called, but the stub saw $( cat "$state/calls.log" )"
    rc=1
  fi

  # 7. wrong-version: a tarball that is not the version being published.
  state="$(_selftest_state wrong-version)"
  dist="$tmp/dist-wrong-version"
  _selftest_dist "$dist"
  rm -f -- "$dist/$(tarball_name_for "$MAIN_NAME" "$SELFTEST_VERSION")"
  _selftest_tarball "$dist" "$MAIN_NAME" "9.9.9"
  out="$(_selftest_publish "$state" "$dist" 1)"
  _selftest_case "wrong-version" rejected "$out" "$?" || rc=1
  _selftest_has "wrong-version" "expected version $SELFTEST_VERSION" "$out" || rc=1

  # 8. missing-platform: a platform package the advertised matrix requires is
  #    not in the set.
  state="$(_selftest_state missing-platform)"
  dist="$tmp/dist-missing-platform"
  _selftest_dist "$dist"
  rm -f -- "$dist/$(tarball_name_for "$MAIN_NAME-${platforms[1]}" "$SELFTEST_VERSION")"
  out="$(_selftest_publish "$state" "$dist" 1)"
  _selftest_case "missing-platform" rejected "$out" "$?" || rc=1
  _selftest_has "missing-platform" "missing platform tarball for suffix ${platforms[1]}" "$out" || rc=1

  # 9. foreign-name: a tarball whose package is not part of this release.
  state="$(_selftest_state foreign-name)"
  dist="$tmp/dist-foreign-name"
  _selftest_dist "$dist"
  _selftest_foreign_tarball "$dist"
  out="$(_selftest_publish "$state" "$dist" 1)"
  _selftest_case "foreign-name" rejected "$out" "$?" || rc=1
  _selftest_has "foreign-name" "unexpected package name @pulsehive/not-this-sdk" "$out" || rc=1

  # 10. post-publish-mismatch: the registry's bytes after a publish are not the
  #     tested bytes — the run refuses rather than reporting success.
  state="$(_selftest_state post-publish-mismatch)"
  dist="$tmp/dist-post-publish-mismatch"
  _selftest_dist "$dist"
  export STUB_LIE_AFTER_PUBLISH_FOR="$MAIN_NAME-${platforms[0]}@$SELFTEST_VERSION"
  out="$(_selftest_publish "$state" "$dist" 1)"
  case_rc=$?
  unset STUB_LIE_AFTER_PUBLISH_FOR
  _selftest_case "post-publish-mismatch" rejected "$out" "$case_rc" || rc=1
  _selftest_has "post-publish-mismatch" "was published but npm reports" "$out" || rc=1

  # 11. fresh-build-hint: the same different-bytes refusal on a
  #     workflow_dispatch run names the remedy (L4 narrowed).
  state="$(_selftest_state fresh-build-hint)"
  dist="$tmp/dist-fresh-build-hint"
  _selftest_dist "$dist"
  _selftest_seed "$state" "$tampered_tarball"
  out="$(_selftest_publish "$state" "$dist" 1 --fresh-build)"
  _selftest_case "fresh-build-hint" rejected "$out" "$?" || rc=1
  _selftest_has "fresh-build-hint" \
    "— this is a fresh build; re-run the failed jobs of the original tag run instead" "$out" || rc=1

  if [ "$rc" -ne 0 ]; then
    return 1
  fi
  say "self-test: ok"
  return 0
}

# --------------------------------------------------------------------- main ---

main() {
  [ "$#" -gt 0 ] || { usage >&2; return 2; }
  case "$1" in
    probe)
      shift
      cmd_probe "$@"
      ;;
    plan)
      shift
      cmd_plan "$@"
      ;;
    publish)
      shift
      cmd_publish "$@"
      ;;
    --self-test)
      run_self_test
      ;;
    -h | --help)
      usage
      ;;
    *)
      error_line "unknown argument '$1' (use probe, plan, publish or --self-test)"
      return 2
      ;;
  esac
}

main "$@"
