#!/usr/bin/env bash
# release-check.sh — tree consistency for one v* release (r2.s3.w1).
#
# release-manifest.toml is the release engineer's tracked input: the version each
# registry publishes for the tag being cut (RELEASE.md, version and tag contract;
# ADR-018). This script fails on any disagreement between that file and the
# package files it names, with one named line on stderr
# (`release-check: ERROR: ...`) so the release runbook can map each to a
# recovery action.
#
#   bash scripts/release-check.sh
#       tree consistency; prints `release-check: ok <workspace.version>`.
#   bash scripts/release-check.sh --ref <github-ref>
#       the above, plus the ref must be `refs/tags/v<workspace.version>` — the
#       tag being cut names the manifest version and no other.
#   bash scripts/release-check.sh --on-main <commit> [--main-ref <ref>]
#       the above, plus <commit> must be an ancestor of `origin/main`
#       (`git merge-base --is-ancestor`); --main-ref overrides that ref.
#   bash scripts/release-check.sh --get <key>
#       print one manifest value: workspace.version, rust.version, rust.crates,
#       npm.version, npm.package, python.version, python.distribution.
#   bash scripts/release-check.sh --self-test
#       hermetic proof, under mktemp -d, that every rejection above fires with
#       its named error and that a consistent tree passes.
#
# RELEASE_CHECK_ROOT overrides the tree root the checks read (internal seam: the
# self-test points the same checks at throwaway trees under mktemp -d).
set -u

PROG="release-check"
ROOT="${RELEASE_CHECK_ROOT:-$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)}"
MANIFEST="$ROOT/release-manifest.toml"
SELF="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)/$(basename -- "${BASH_SOURCE[0]}")"

# Values of release-manifest.toml, flattened by load_manifest into `key<TAB>value`.
declare -A M

# --main-ref default; the ref `--on-main` proves ancestry against.
MAIN_REF="origin/main"

# The self-test's throwaway tree; global so the EXIT trap still sees it.
SELFTEST_TMP=""

usage() {
  cat <<EOF
usage:
  $PROG [--ref <github-ref>] [--on-main <commit> [--main-ref <ref>]]
  $PROG --get <key>
  $PROG --self-test
EOF
}

fail() {
  echo "$PROG: ERROR: $*" >&2
  exit 1
}

# load_manifest — parse the manifest through tomllib and print a `key<TAB>value`
# dump of every value the checks compare. Exits 1 with a named error when the
# file is missing, does not parse, or lacks a key. Nested manifests (Cargo.toml,
# package.json, pyproject.toml) are read the same way, by toml_get/json_get.
load_manifest() {
  python3 -c '
import sys
import tomllib

prog, path = sys.argv[1], sys.argv[2]
REQUIRED = (
    "workspace.version",
    "rust.version",
    "rust.crates",
    "npm.version",
    "npm.package",
    "python.version",
    "python.distribution",
)


def value_at(data, dotted):
    node = data
    for part in dotted.split("."):
        if not isinstance(node, dict) or part not in node:
            return None
        node = node[part]
    return node


def flat(value):
    if isinstance(value, list):
        return " ".join(str(item) for item in value)
    return str(value)


def die(message):
    print(f"{prog}: ERROR: {message}", file=sys.stderr)
    sys.exit(1)


try:
    with open(path, "rb") as handle:
        data = tomllib.load(handle)
except FileNotFoundError:
    die(f"manifest {path} not found")
except tomllib.TOMLDecodeError as exc:
    die(f"manifest {path} does not parse: {exc}")

schema = data.get("schema")
if not isinstance(schema, int) or schema != 1:
    die(f"manifest schema is {schema!r}, expected 1")

for key in REQUIRED:
    if value_at(data, key) is None:
        die(f"manifest {path} is missing {key}")

print(f"schema\t{schema}")
for key in REQUIRED:
    print(f"{key}\t{flat(value_at(data, key))}")
' "$PROG" "$MANIFEST"
}

read_manifest() {
  local dump m_key m_value
  if ! dump="$(load_manifest)"; then
    exit 1 # the named error is already on stderr
  fi
  while IFS=$'\t' read -r m_key m_value; do
    if [ -n "$m_key" ]; then M["$m_key"]="$m_value"; fi
  done <<<"$dump"
}

# toml_get <file> <dotted key> — print a scalar (or space-joined list) value of a
# TOML file; print nothing and exit 0 when the key is absent, exit 2 when the
# file cannot be read or parsed.
toml_get() {
  python3 -c '
import sys
import tomllib

path, dotted = sys.argv[1], sys.argv[2]
try:
    with open(path, "rb") as handle:
        node = tomllib.load(handle)
except (OSError, tomllib.TOMLDecodeError):
    sys.exit(2)
for part in dotted.split("."):
    if not isinstance(node, dict) or part not in node:
        sys.exit(0)
    node = node[part]
if isinstance(node, list):
    print(" ".join(str(item) for item in node))
else:
    print(node)
' "$1" "$2"
}

# json_get <file> <dotted key> — the package.json counterpart of toml_get.
json_get() {
  python3 -c '
import json
import sys

path, dotted = sys.argv[1], sys.argv[2]
try:
    with open(path, encoding="utf-8") as handle:
        node = json.load(handle)
except (OSError, json.JSONDecodeError):
    sys.exit(2)
for part in dotted.split("."):
    if not isinstance(node, dict) or part not in node:
        sys.exit(0)
    node = node[part]
print(node)
' "$1" "$2"
}

# check_crate_versions — every crate in rust.crates declares rust.version, and
# every `[workspace.dependencies]` entry the root manifest holds for one of them
# carries the same version (or the `=version` pin of it). The meta-crate has no
# such entry, so an absent one is not itself a failure — the build resolves
# path dependencies without it.
check_crate_versions() {
  local crate found
  for crate in ${M[rust.crates]}; do
    found="$(toml_get "$ROOT/$crate/Cargo.toml" package.version)" ||
      fail "$crate/Cargo.toml: no readable [package] version"
    [ "$found" = "${M[rust.version]}" ] ||
      fail "$crate/Cargo.toml declares $found, manifest rust.version is ${M[rust.version]}"

    found="$(toml_get "$ROOT/Cargo.toml" "workspace.dependencies.$crate.version")" ||
      fail "Cargo.toml: no readable [workspace.dependencies].$crate.version"
    [ -n "$found" ] || continue
    case "$found" in
      "${M[rust.version]}" | "=${M[rust.version]}") : ;;
      *) fail "Cargo.toml declares $found, manifest rust.version is ${M[rust.version]}" ;;
    esac
  done
}

# check_binding_versions — the two language bindings agree with their registry
# line, including the names the registries key on.
check_binding_versions() {
  local found
  found="$(toml_get "$ROOT/pulsehive-js/Cargo.toml" package.version)" ||
    fail "pulsehive-js/Cargo.toml: no readable [package] version"
  [ "$found" = "${M[npm.version]}" ] ||
    fail "pulsehive-js/Cargo.toml declares $found, manifest npm.version is ${M[npm.version]}"

  found="$(json_get "$ROOT/pulsehive-js/package.json" version)" ||
    fail "pulsehive-js/package.json: no readable version"
  [ "$found" = "${M[npm.version]}" ] ||
    fail "pulsehive-js/package.json declares $found, manifest npm.version is ${M[npm.version]}"

  found="$(json_get "$ROOT/pulsehive-js/package.json" name)" ||
    fail "pulsehive-js/package.json: no readable name"
  [ "$found" = "${M[npm.package]}" ] ||
    fail "pulsehive-js/package.json declares $found, manifest npm.package is ${M[npm.package]}"

  found="$(toml_get "$ROOT/pulsehive-py/Cargo.toml" package.version)" ||
    fail "pulsehive-py/Cargo.toml: no readable [package] version"
  [ "$found" = "${M[python.version]}" ] ||
    fail "pulsehive-py/Cargo.toml declares $found, manifest python.version is ${M[python.version]}"

  found="$(toml_get "$ROOT/pulsehive-py/pyproject.toml" project.name)" ||
    fail "pulsehive-py/pyproject.toml: no readable [project] name"
  [ "$found" = "${M[python.distribution]}" ] ||
    fail "pulsehive-py/pyproject.toml declares $found, manifest python.distribution is ${M[python.distribution]}"
}

# check_tree — the whole consistency contract for the manifest and the package
# files it names: the joined-line rule first (ADR-015 L1 / ADR-016 L1 hold the
# three registry lines to the workspace line while they stand), then each file.
check_tree() {
  local line
  for line in rust npm python; do
    [ "${M[$line.version]}" = "${M[workspace.version]}" ] ||
      fail "$line.version ${M[$line.version]} differs from workspace.version ${M[workspace.version]} (joined line, ADR-015 L1 / ADR-016 L1)"
  done
  check_crate_versions
  check_binding_versions
}

# check_ref <github-ref> — the ref the release runs on must be exactly the
# manifest version's tag; a v* tag naming any other version is a mismatch, and
# a non-tag ref is a different failure with its own name.
check_ref() {
  local ref="$1" tag
  case "$ref" in
    refs/tags/v*) tag="${ref#refs/tags/}" ;;
    *) fail "ref $ref is not a v* tag" ;;
  esac
  [ "$tag" = "v${M[workspace.version]}" ] ||
    fail "tag $tag does not name manifest version ${M[workspace.version]}"
}

# check_on_main <commit> — the commit a release runs on must be an ancestor of
# MAIN_REF. Fail-closed: an unreadable ref or a missing repository reads the
# same as a commit that is genuinely off main.
check_on_main() {
  git -C "$ROOT" merge-base --is-ancestor "$1" "$MAIN_REF" ||
    fail "commit $1 is not on $MAIN_REF"
}

# print_value <key> — one manifest value, the value alone.
print_value() {
  case "$1" in
    workspace.version | rust.version | rust.crates | npm.version | npm.package | python.version | python.distribution)
      printf '%s\n' "${M[$1]}"
      ;;
    *) fail "unknown manifest key '$1'" ;;
  esac
}

# ---------------------------------------------------------------------------
# --self-test
#
# Every case runs the shipped script — same file, same argument parsing, same
# exit codes — against a throwaway tree with RELEASE_CHECK_ROOT pointing at it.
# Fixture values come from the shipped manifest, so a version bump moves them
# with it; every mutation is asserted to have applied, so a fixture that stops
# being a mutation fails the self-test instead of passing vacuously.
# ---------------------------------------------------------------------------

SELFTEST_OUT=""
SELFTEST_RC=0

selftest_fail() { # <label> <detail>
  echo "self-test: FAILED [$1]: $2" >&2
  exit 1
}

# selftest_write_tree <dir> [<drift-crate> <drift-version>] — a copy of the
# shipped manifest plus minimal stand-ins matching it; the optional pair drifts
# one crate's `[workspace.dependencies]` requirement.
selftest_write_tree() {
  local dir="$1" drift_crate="${2:-}" drift_version="${3:-}"
  local crates="${M[rust.crates]}" v_rust="${M[rust.version]}"
  local v_npm="${M[npm.version]}" v_py="${M[python.version]}"
  local crate version

  mkdir -p "$dir"
  cp "$MANIFEST" "$dir/release-manifest.toml"

  for crate in $crates; do
    mkdir -p "$dir/$crate"
    cat >"$dir/$crate/Cargo.toml" <<EOF
[package]
name = "$crate"
version = "$v_rust"
EOF
  done

  {
    printf '[workspace.dependencies]\n'
    for crate in $crates; do
      version="$v_rust"
      if [ "$crate" = "$drift_crate" ]; then version="$drift_version"; fi
      printf '%s = { path = "%s", version = "%s" }\n' "$crate" "$crate" "$version"
    done
  } >"$dir/Cargo.toml"

  mkdir -p "$dir/pulsehive-js"
  cat >"$dir/pulsehive-js/Cargo.toml" <<EOF
[package]
name = "pulsehive-js"
version = "$v_npm"
EOF
  cat >"$dir/pulsehive-js/package.json" <<EOF
{
  "name": "${M[npm.package]}",
  "version": "$v_npm"
}
EOF

  mkdir -p "$dir/pulsehive-py"
  cat >"$dir/pulsehive-py/Cargo.toml" <<EOF
[package]
name = "pulsehive-py"
version = "$v_py"
EOF
  cat >"$dir/pulsehive-py/pyproject.toml" <<EOF
[project]
name = "${M[python.distribution]}"
EOF
}

# selftest_init_repo <dir> — a throwaway repository whose `main` holds the tree
# and whose `topic` branch is one commit ahead of it, so `--on-main` has a real
# not-an-ancestor case. Built with `git fast-import`, which needs no committer
# identity and runs no hooks — the fixture is hermetic on any host git config.
selftest_init_repo() {
  local dir="$1"
  git -C "$dir" init -q
  git -C "$dir" fast-import --quiet <<'FIXTURE_GIT'
commit refs/heads/main
committer release-check self-test <selftest@example.invalid> 0 +0000
data <<MSG
fixture base
MSG

commit refs/heads/topic
committer release-check self-test <selftest@example.invalid> 0 +0000
data <<MSG
fixture topic
MSG
from refs/heads/main

FIXTURE_GIT
  git -C "$dir" update-ref refs/remotes/origin/main refs/heads/main
}

# selftest_edit_manifest <file> <set|drop> <section> <key> [value] — the
# smallest line-level TOML edit the two manifest fixtures need (tomllib is
# read-only). Exits non-zero when the edit does not apply, so reformatting the
# manifest cannot turn a fixture into a silent no-op.
selftest_edit_manifest() {
  python3 -c '
import sys

path, mode, section, key = sys.argv[1:5]
value = sys.argv[5] if len(sys.argv) > 5 else None
with open(path, encoding="utf-8") as handle:
    lines = handle.read().splitlines(keepends=True)
out, current, applied = [], "", False
for line in lines:
    stripped = line.strip()
    if stripped.startswith("[") and stripped.endswith("]"):
        current = stripped[1:-1]
    elif current == section and not stripped.startswith("#"):
        name, sep, _ = stripped.partition("=")
        if sep and name.strip() == key:
            applied = True
            if mode == "set":
                out.append(f"{key} = \"{value}\"\n")
            continue
    out.append(line)
if not applied:
    print(f"fixture edit did not apply: [{section}] {key} in {path}", file=sys.stderr)
    sys.exit(1)
with open(path, "w", encoding="utf-8") as handle:
    handle.writelines(out)
' "$@"
}

selftest_expect_rejected() { # <label> <tree> <named error> [args...]
  local label="$1" tree="$2" want="$3"
  shift 3
  SELFTEST_OUT="$(RELEASE_CHECK_ROOT="$tree" bash "$SELF" "$@" 2>&1)"
  SELFTEST_RC=$?
  [ "$SELFTEST_RC" -ne 0 ] ||
    selftest_fail "$label" "expected a rejection, got rc=0: $SELFTEST_OUT"
  case "$SELFTEST_OUT" in
    *"$want"*) ;;
    *) selftest_fail "$label" "rejected (rc=$SELFTEST_RC) without the named error ('$want'): $SELFTEST_OUT" ;;
  esac
  echo "case $label: rejected"
}

selftest_expect_accepted() { # <label> <tree> <success line> [args...]
  local label="$1" tree="$2" want="$3"
  shift 3
  SELFTEST_OUT="$(RELEASE_CHECK_ROOT="$tree" bash "$SELF" "$@" 2>&1)"
  SELFTEST_RC=$?
  [ "$SELFTEST_RC" -eq 0 ] ||
    selftest_fail "$label" "expected acceptance, got rc=$SELFTEST_RC: $SELFTEST_OUT"
  case "$SELFTEST_OUT" in
    *"$want"*) ;;
    *) selftest_fail "$label" "accepted (rc=0) without the success line ('$want'): $SELFTEST_OUT" ;;
  esac
  echo "case $label: accepted"
}

self_test() {
  local v_ws="${M[workspace.version]}" v_rust="${M[rust.version]}"
  local v_npm="${M[npm.version]}" v_py="${M[python.version]}"
  local npm_name="${M[npm.package]}" py_name="${M[python.distribution]}"
  local first_crate="${M[rust.crates]%% *}" last_crate="${M[rust.crates]##* }"
  local drift="0.0.0" tree good="$SELFTEST_TMP/good"
  if [ "$drift" = "$v_ws" ] || [ "$drift" = "$v_rust" ] || [ "$drift" = "$v_npm" ] || [ "$drift" = "$v_py" ]; then
    fail "self-test: fixture drift version $drift collides with a manifest version"
  fi

  selftest_write_tree "$good"
  selftest_init_repo "$good" || fail "self-test: cannot build the fixture repository"

  # The ancestry cases only mean something while the fixture really holds a
  # commit on main and one off it: check_on_main fails closed on an unreadable
  # repository, so a broken fixture would otherwise read as a rejection.
  git -C "$good" merge-base --is-ancestor refs/heads/main refs/remotes/origin/main ||
    selftest_fail "commit-not-on-main" "fixture main is not an ancestor of its origin/main"
  if git -C "$good" merge-base --is-ancestor refs/heads/topic refs/remotes/origin/main; then
    selftest_fail "commit-not-on-main" "fixture topic is an ancestor of origin/main — nothing to reject"
  fi

  selftest_expect_rejected "mismatched-tag" "$good" \
    "$PROG: ERROR: tag v$drift does not name manifest version $v_ws" \
    --ref "refs/tags/v$drift"
  selftest_expect_rejected "non-tag-ref" "$good" \
    "$PROG: ERROR: ref refs/heads/main is not a v* tag" \
    --ref refs/heads/main
  selftest_expect_rejected "commit-not-on-main" "$good" \
    "$PROG: ERROR: commit refs/heads/topic is not on origin/main" \
    --ref "refs/tags/v$v_ws" --on-main refs/heads/topic

  tree="$SELFTEST_TMP/drifted-crate-version"
  selftest_write_tree "$tree"
  cat >"$tree/$last_crate/Cargo.toml" <<EOF
[package]
name = "$last_crate"
version = "$drift"
EOF
  selftest_expect_rejected "drifted-crate-version" "$tree" \
    "$PROG: ERROR: $last_crate/Cargo.toml declares $drift, manifest rust.version is $v_rust"

  tree="$SELFTEST_TMP/drifted-crate-requirement"
  selftest_write_tree "$tree" "$first_crate" "$drift"
  selftest_expect_rejected "drifted-crate-requirement" "$tree" \
    "$PROG: ERROR: Cargo.toml declares $drift, manifest rust.version is $v_rust"

  tree="$SELFTEST_TMP/drifted-package-version"
  selftest_write_tree "$tree"
  cat >"$tree/pulsehive-js/package.json" <<EOF
{
  "name": "$npm_name",
  "version": "$drift"
}
EOF
  selftest_expect_rejected "drifted-package-version" "$tree" \
    "$PROG: ERROR: pulsehive-js/package.json declares $drift, manifest npm.version is $v_npm"

  tree="$SELFTEST_TMP/drifted-package-name"
  selftest_write_tree "$tree"
  cat >"$tree/pulsehive-js/package.json" <<EOF
{
  "name": "pulsehive-sdk-drift",
  "version": "$v_npm"
}
EOF
  selftest_expect_rejected "drifted-package-name" "$tree" \
    "$PROG: ERROR: pulsehive-js/package.json declares pulsehive-sdk-drift, manifest npm.package is $npm_name"

  tree="$SELFTEST_TMP/drifted-python-crate-version"
  selftest_write_tree "$tree"
  cat >"$tree/pulsehive-py/Cargo.toml" <<EOF
[package]
name = "pulsehive-py"
version = "$drift"
EOF
  selftest_expect_rejected "drifted-python-crate-version" "$tree" \
    "$PROG: ERROR: pulsehive-py/Cargo.toml declares $drift, manifest python.version is $v_py"

  tree="$SELFTEST_TMP/drifted-pyproject-name"
  selftest_write_tree "$tree"
  cat >"$tree/pulsehive-py/pyproject.toml" <<EOF
[project]
name = "pulsehive-drift"
EOF
  selftest_expect_rejected "drifted-pyproject-name" "$tree" \
    "$PROG: ERROR: pulsehive-py/pyproject.toml declares pulsehive-drift, manifest python.distribution is $py_name"

  tree="$SELFTEST_TMP/joined-line-violation"
  selftest_write_tree "$tree"
  selftest_edit_manifest "$tree/release-manifest.toml" set npm version "$drift" ||
    selftest_fail "joined-line-violation" "fixture edit did not apply"
  selftest_expect_rejected "joined-line-violation" "$tree" \
    "$PROG: ERROR: npm.version $drift differs from workspace.version $v_ws (joined line, ADR-015 L1 / ADR-016 L1)"

  tree="$SELFTEST_TMP/missing-manifest-key"
  selftest_write_tree "$tree"
  selftest_edit_manifest "$tree/release-manifest.toml" drop rust version ||
    selftest_fail "missing-manifest-key" "fixture edit did not apply"
  selftest_expect_rejected "missing-manifest-key" "$tree" \
    "$PROG: ERROR: manifest $tree/release-manifest.toml is missing rust.version"

  selftest_expect_accepted "consistent-tree" "$good" "$PROG: ok $v_ws" \
    --ref "refs/tags/v$v_ws" --on-main refs/heads/main
  selftest_expect_accepted "main-ref-override" "$good" "$PROG: ok $v_ws" \
    --on-main refs/heads/topic --main-ref refs/heads/topic

  echo "self-test: ok"
}

main() {
  local ref="" on_main="" key="" with_self_test=0

  while [ $# -gt 0 ]; do
    case "$1" in
      --ref)
        [ $# -ge 2 ] || {
          usage >&2
          exit 2
        }
        ref="$2"
        shift 2
        ;;
      --on-main)
        [ $# -ge 2 ] || {
          usage >&2
          exit 2
        }
        on_main="$2"
        shift 2
        ;;
      --main-ref)
        [ $# -ge 2 ] || {
          usage >&2
          exit 2
        }
        MAIN_REF="$2"
        shift 2
        ;;
      --get)
        [ $# -ge 2 ] || {
          usage >&2
          exit 2
        }
        key="$2"
        shift 2
        ;;
      --self-test)
        with_self_test=1
        shift
        ;;
      -h | --help)
        usage
        exit 0
        ;;
      *)
        usage >&2
        echo "$PROG: unknown argument '$1'" >&2
        exit 2
        ;;
    esac
  done

  read_manifest

  if [ "$with_self_test" -eq 1 ]; then
    SELFTEST_TMP="$(mktemp -d "${TMPDIR:-/tmp}/release-check-selftest.XXXXXX")" ||
      fail "self-test: cannot create a temp dir"
    trap 'rm -rf "${SELFTEST_TMP:-}"' EXIT
    self_test
    exit 0
  fi

  if [ -n "$key" ]; then
    print_value "$key"
    exit 0
  fi

  check_tree
  [ -z "$ref" ] || check_ref "$ref"
  [ -z "$on_main" ] || check_on_main "$on_main"
  echo "$PROG: ok ${M[workspace.version]}"
}

main "$@"
