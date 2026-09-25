#!/usr/bin/env bash
# py-wheel-smoke.sh — hermetic wheel proof for the `pulsehive` Python
# distribution (r2.s2.w3). Builds the wheel this tree would publish, installs it
# into a fresh virtualenv with --no-index (offline, no dependency resolution —
# installing a wheel never needs a build toolchain), imports `pulsehive`, and
# asserts the version and the cp311-abi3 tag that ADR-016 promises. Demo line
# d23; the single install path w4 wires into CI.
#
# Modes:
#   (default)           build the wheel from pulsehive-py/ with maturin, then run
#                       the install-and-assert half. Prints `py wheel smoke: ok`
#                       as its last line on success.
#   --wheel <path>      the same install-and-assert half against an already-built
#                       wheel — no build step (w4's CI matrix legs call this).
#   --self-test         hermetic (no network): plants tampered wheels whose
#                       metadata version and tags disagree and requires this
#                       script's own checks to reject each, plus a live
#                       import-path control against the tampered wheel. Prints
#                       `self-test: ok` as its last line.
#
# Build profile (implementer's choice, sized for the warm < 240 s bound against
# RELEASE.md's 600 s budget): maturin itself is installed into a throwaway venv
# under the run's mktemp dir; cargo compilation lands in a persistent target dir
# OUTSIDE the worktree ($PULSEHIVE_WHEEL_SMOKE_TARGET_DIR, default
# ~/.cache/pulsehive/py-wheel-smoke/target) so the cold ort-sys/onnxruntime
# fetch is paid once and later runs stay warm. The wheel, the virtualenvs and
# every other temp file live under one mktemp -d dir that the EXIT trap removes.
# Nothing this script creates is ever written inside the worktree; a workspace
# Cargo.lock the build generates (gitignored in this repo) is removed again on
# exit. The asserted version is read from pulsehive-py/Cargo.toml — never a
# restated copy (r2.s1 L4). This item proves an artifact; it never uploads one.
#
# Interpreter rule (every mode): $PYTHON if set, else python3, else python, and
# it must be >= 3.11 (ADR-016 requires-python) or the script fails with a named
# error.
#
# venv rule (every mode): a virtualenv is resolved where the interpreter that
# created it puts it — `bin/python` and `bin/<script>` on POSIX, `Scripts/
# python.exe` and `Scripts/<script>.exe` on Windows (both the release matrix's
# windows-latest leg and the local Git Bash there). Nothing here assumes the
# POSIX layout: a layout this host does not have is simply not the one that is
# used, so the same script proves the same wheel on all three advertised
# targets.

set -u

PROG="py-wheel-smoke"
PY_TAG="cp311"      # ADR-016: requires-python >=3.11 on the stable ABI
ABI_TAG="abi3"
MIN_MAJOR=3
MIN_MINOR=11
STALE_VER="0.3.0b2" # the stale version issue #92 forbids; a fixture, never asserted

PY=""
ROOT=""
MANIFEST=""
PYPROJECT=""
TARGET_DIR=""
SMOKE_TMP=""
VENV_PY=""
LOCK_GENERATED=0

usage() {
  cat <<EOF
usage:
  $PROG                  build the wheel, then install-and-assert it in a fresh venv
  $PROG --wheel <path>   install-and-assert an already-built wheel (no build)
  $PROG --self-test      hermetic proof that the assertions are live
EOF
}

fail() {
  echo "$PROG: ERROR: $*" >&2
  exit 1
}

# resolve_py — set PY to $PYTHON, else python3, else python; named error if none
resolve_py() {
  if [ -n "${PYTHON:-}" ]; then
    command -v "$PYTHON" >/dev/null 2>&1 ||
      fail "\$PYTHON='$PYTHON' is set but is not a runnable interpreter"
    PY="$PYTHON"
    return 0
  fi
  if command -v python3 >/dev/null 2>&1; then
    PY="python3"
  elif command -v python >/dev/null 2>&1; then
    PY="python"
  else
    fail "no Python interpreter found (looked for \$PYTHON, python3, python)"
  fi
}

# check_py_version — the resolved interpreter must be >= 3.11 (ADR-016)
check_py_version() {
  local v major minor
  v=$("$PY" -c 'import sys; print("%d.%d" % sys.version_info[:2])') ||
    fail "cannot query the Python version of interpreter '$PY'"
  major=${v%%.*}
  minor=${v#*.}
  minor=${minor%%.*}
  case "${major}${minor}" in
    *[!0-9]*) fail "interpreter '$PY' reported a non-numeric version '$v'" ;;
  esac
  if [ "$major" -lt "$MIN_MAJOR" ] ||
    { [ "$major" -eq "$MIN_MAJOR" ] && [ "$minor" -lt "$MIN_MINOR" ]; }; then
    fail "interpreter '$PY' is Python $v — the distribution requires >= ${MIN_MAJOR}.${MIN_MINOR} (ADR-016 requires-python)"
  fi
}

# manifest_version — the literal version under [package] in the crate manifest
manifest_version() {
  local v
  v=$(awk -F= '/^\[/{inpkg=($0=="[package]"); next} inpkg && $1 ~ /^version[[:space:]]*$/ {gsub(/[[:space:]"]/, "", $2); print $2; exit}' "$MANIFEST")
  [ -n "$v" ] || fail "cannot read the version from $MANIFEST — expected a literal version under [package]"
  printf '%s' "$v"
}

# project_name — the distribution name from pyproject [project] (not restated)
project_name() {
  local v
  v=$(awk -F= '/^\[/{inp=($0=="[project]"); next} inp && $1 ~ /^name[[:space:]]*$/ {gsub(/[[:space:]"]/, "", $2); print $2; exit}' "$PYPROJECT")
  [ -n "$v" ] || fail "cannot read the distribution name from $PYPROJECT — expected a name under [project]"
  printf '%s' "$v"
}

# wheel_file_faults — judge a wheel file against the manifest version and the
# ADR-016 tags: filename shape, name, version, python/abi tag, and the versions
# and tags the wheel's own metadata claims. Prints fault lines; rc 0 iff none.
wheel_file_faults() { # <wheel> <expect-version> <name>
  local wheel="$1" ver="$2" name="$3" base meta wheel_tags
  [ -f "$wheel" ] || { echo "wheel file not found: '$wheel'"; return 1; }
  base=${wheel##*/}
  base=${base%.whl}
  local fields=()
  IFS='-' read -r -a fields <<< "$base"
  if [ "${#fields[@]}" -ne 5 ]; then
    echo "wheel filename is not <name>-<version>-<python>-<abi>-<platform>: '$base'"
    return 1
  fi
  [ "${fields[0]}" = "$name" ] ||
    { echo "wheel distribution name '${fields[0]}' != '$name' (pyproject [project] name)"; return 1; }
  [ "${fields[1]}" = "$ver" ] ||
    { echo "wheel filename carries version '${fields[1]}' but $MANIFEST declares '$ver'"; return 1; }
  [ "${fields[2]}" = "$PY_TAG" ] ||
    { echo "wheel python tag '${fields[2]}' != '$PY_TAG' (ADR-016: >=3.11 on the stable ABI)"; return 1; }
  [ "${fields[3]}" = "$ABI_TAG" ] ||
    { echo "wheel abi tag '${fields[3]}' != '$ABI_TAG'"; return 1; }
  meta=$(unzip -p "$wheel" '*.dist-info/METADATA' 2>/dev/null | awk '/^Version:/{print $2; exit}')
  [ "$meta" = "$ver" ] ||
    { echo "wheel METADATA version '${meta:-<missing>}' != '$ver' declared by $MANIFEST"; return 1; }
  wheel_tags=$(unzip -p "$wheel" '*.dist-info/WHEEL' 2>/dev/null | grep -c "^Tag: ${PY_TAG}-${ABI_TAG}-")
  [ "${wheel_tags:-0}" -ge 1 ] ||
    { echo "wheel metadata carries no 'Tag: ${PY_TAG}-${ABI_TAG}-' entry"; return 1; }
  return 0
}

# venv_python — the interpreter a virtualenv created under <dir> exposes:
# `Scripts/python.exe` on Windows, `bin/python` elsewhere. Prints the path; rc 1
# when the dir holds neither layout.
venv_python() { # <venv-dir>
  if [ -f "$1/Scripts/python.exe" ]; then
    printf '%s\n' "$1/Scripts/python.exe"
  elif [ -x "$1/bin/python" ] || [ -f "$1/bin/python" ]; then
    printf '%s\n' "$1/bin/python"
  else
    return 1
  fi
}

# venv_script — a console script that the venv's pip installed for <name>:
# `Scripts/<name>.exe` on Windows, `bin/<name>` elsewhere. Prints the path; rc 1
# when the venv holds neither layout (maturin is the caller that matters).
venv_script() { # <venv-dir> <name>
  if [ -f "$1/Scripts/$2.exe" ]; then
    printf '%s\n' "$1/Scripts/$2.exe"
  elif [ -x "$1/bin/$2" ] || [ -f "$1/bin/$2" ]; then
    printf '%s\n' "$1/bin/$2"
  else
    return 1
  fi
}

# make_venv — create a virtualenv under <dir>; works with or without ensurepip
# (Debian images ship python3 without it) by falling back to --without-pip with
# the system pip left visible. Sets VENV_PY to the venv's interpreter, resolved
# from whichever layout this host's venv uses.
make_venv() { # <dir>
  local vp
  if "$PY" -m venv "$1" >/dev/null 2>&1 &&
    vp=$(venv_python "$1") &&
    "$vp" -m pip --version >/dev/null 2>&1; then
    VENV_PY="$vp"
    return 0
  fi
  rm -rf "$1"
  if ! "$PY" -m venv --without-pip --system-site-packages "$1" >/dev/null 2>&1; then
    return 1
  fi
  vp=$(venv_python "$1") || return 1
  if ! "$vp" -m pip --version >/dev/null 2>&1; then
    return 1
  fi
  VENV_PY="$vp"
  return 0
}

# pip_install_offline — install one wheel into the venv with --no-index (no
# network, no dependency resolution, no toolchain). Prefers the venv's own pip;
# falls back to the driver interpreter's pip via pip's --python (pip >= 22.3)
# when the venv was created --without-pip.
pip_install_offline() { # <venv-python> <wheel> [extra pip args...]
  local py="$1" wheel="$2"
  shift 2
  if "$py" -m pip --version >/dev/null 2>&1; then
    "$py" -m pip install "$@" --no-index --no-deps --disable-pip-version-check --quiet "$wheel"
  else
    "$PY" -m pip --python "$py" install "$@" --no-index --no-deps --disable-pip-version-check --quiet "$wheel"
  fi
}

# import_version_matches — import pulsehive in the venv and require the imported
# distribution to report <expect-version>. Fault text on stdout; rc 0 iff equal.
import_version_matches() { # <venv-python> <expect-version>
  local got
  got=$("$1" -c 'import pulsehive, importlib.metadata as m; print(m.version("pulsehive"))' 2>&1) ||
    { echo "importing 'pulsehive' in the fresh virtualenv failed: $got"; return 1; }
  [ "$got" = "$2" ] ||
    { echo "imported 'pulsehive' reports version '$got', expected '$2'"; return 1; }
  return 0
}

# install_and_import — the install-and-assert half shared by the default and
# --wheel modes: file-level checks, fresh venv, offline install, import.
install_and_import() { # <wheel> <expect-version> <name> <tmpdir>
  local wheel="$1" ver="$2" name="$3" tmp="$4" faults out
  faults=$(wheel_file_faults "$wheel" "$ver" "$name") ||
    fail "wheel '$(basename -- "$wheel")' rejected: $faults"
  make_venv "$tmp/consumer-venv" ||
    fail "cannot create a fresh virtualenv under '$tmp' with interpreter '$PY'"
  pip_install_offline "$VENV_PY" "$wheel" ||
    fail "offline install of '$(basename -- "$wheel")' failed (--no-index)"
  out=$(import_version_matches "$VENV_PY" "$ver") || fail "$out"
}

# build_wheel — build the distribution from pulsehive-py/ with maturin and print
# the single wheel path. Only this half may touch the network (installing
# maturin) and cargo; compilation goes to $TARGET_DIR, the wheel to <dist-dir>.
build_wheel() { # <dist-dir>
  local dist="$1" mv="$SMOKE_TMP/maturin-venv" maturin wheel
  make_venv "$mv" || fail "cannot create the maturin virtualenv with interpreter '$PY'"
  "$VENV_PY" -m pip install --disable-pip-version-check --quiet maturin ||
    fail "cannot install maturin into the build venv (only the build half uses the network)"
  maturin=$(venv_script "$mv" maturin) ||
    fail "no maturin launcher in '$mv' — looked for Scripts/maturin.exe (Windows) and bin/maturin (POSIX)"
  mkdir -p "$dist" || fail "cannot create the wheel output dir '$dist'"
  (cd "$ROOT/pulsehive-py" && CARGO_TARGET_DIR="$TARGET_DIR" "$maturin" build --release -o "$dist") ||
    fail "maturin build failed (cargo target dir: '$TARGET_DIR')"
  local wheels=()
  shopt -s nullglob
  wheels=("$dist"/*.whl)
  shopt -u nullglob
  [ "${#wheels[@]}" -eq 1 ] || fail "expected exactly one wheel in '$dist', found ${#wheels[@]}"
  wheel=${wheels[0]}
  echo "$PROG: built $(basename -- "$wheel")" >&2
  printf '%s\n' "$wheel"
}

# plant_wheel — build a synthetic wheel for the self-test: a valid zip (RECORD
# included — pip refuses RECORD-less wheels) whose filename version, METADATA
# version and tags are chosen by the caller, so tampered variants can be planted
# per case. The platform tag is this host's, so pip will actually install it.
plant_wheel() { # <case-dir> <fname-version> <metadata-version> <python-tag> <abi-tag> [wheel-tag]
  "$PY" - "$@" <<'PLANT'
import base64, hashlib, os, sys, sysconfig, zipfile

case_dir, fname_ver, meta_ver, py_tag, abi_tag = sys.argv[1:6]
wheel_tag = sys.argv[6] if len(sys.argv) > 6 else f"{py_tag}-{abi_tag}"
plat = sysconfig.get_platform().replace("-", "_").replace(".", "_")
name = "pulsehive"
os.makedirs(case_dir, exist_ok=True)
path = os.path.join(case_dir, f"{name}-{fname_ver}-{py_tag}-{abi_tag}-{plat}.whl")
di = f"{name}-{meta_ver}.dist-info"
files = [
    (f"{name}/__init__.py", ""),
    (f"{di}/METADATA", f"Metadata-Version: 2.1\nName: {name}\nVersion: {meta_ver}\n"),
    (f"{di}/WHEEL", "Wheel-Version: 1.0\nGenerator: pulsehive wheel smoke self-test\n"
                    f"Root-Is-Purelib: true\nTag: {wheel_tag}-{plat}\n"),
]
def digest(data):
    return base64.urlsafe_b64encode(hashlib.sha256(data).digest()).rstrip(b"=").decode()
with zipfile.ZipFile(path, "w") as z:
    for arc, text in files:
        z.writestr(arc, text)
    record = "".join(f"{arc},{digest(z.read(arc))},{z.getinfo(arc).file_size}\n" for arc, _ in files)
    z.writestr(f"{di}/RECORD", record + f"{di}/RECORD,,\n")
PLANT
}

expect_file_ok() { # <label> <wheel> <ver> <name>
  local out
  out=$(wheel_file_faults "$2" "$3" "$4") || {
    echo "self-test: FAILED [$1]: expected acceptance, got: $out" >&2
    exit 1
  }
}

expect_file_reject() { # <label> <wheel> <ver> <name> <named-error substring>
  local out rc
  out=$(wheel_file_faults "$2" "$3" "$4")
  rc=$?
  if [ "$rc" -eq 0 ]; then
    echo "self-test: FAILED [$1]: expected rejection, but the checks exited 0" >&2
    exit 1
  fi
  case "$out" in
    *"$5"*) : ;;
    *)
      echo "self-test: FAILED [$1]: rejected without the named error ('$5'): $out" >&2
      exit 1
      ;;
  esac
}

expect_import_ok() { # <label> <venv-python> <ver>
  local out
  out=$(import_version_matches "$2" "$3") || {
    echo "self-test: FAILED [$1]: $out" >&2
    exit 1
  }
}

expect_import_reject() { # <label> <venv-python> <ver> <named-error substring>
  local out rc
  out=$(import_version_matches "$2" "$3")
  rc=$?
  if [ "$rc" -eq 0 ]; then
    echo "self-test: FAILED [$1]: expected the import assertion to fire, but it passed" >&2
    exit 1
  fi
  case "$out" in
    *"$4"*) : ;;
    *)
      echo "self-test: FAILED [$1]: failed without the named error ('$4'): $out" >&2
      exit 1
      ;;
  esac
}

expect_resolves_to() { # <label> <expected path> <resolver> <args...>
  local label="$1" want="$2" resolver="$3" got
  shift 3
  got=$("$resolver" "$@") || {
    echo "self-test: FAILED [$label]: $resolver $* resolved nothing" >&2
    exit 1
  }
  [ "$got" = "$want" ] || {
    echo "self-test: FAILED [$label]: $resolver $* -> '$got', expected '$want'" >&2
    exit 1
  }
}

expect_resolves_nothing() { # <label> <resolver> <args...>
  local label="$1" resolver="$2"
  shift 2
  if "$resolver" "$@" >/dev/null 2>&1; then
    echo "self-test: FAILED [$label]: $resolver $* resolved something, expected a failure" >&2
    exit 1
  fi
}

# expect_venv_layouts — the venv layout rule is live for BOTH layouts the
# release matrix meets: the POSIX one this host uses and the Windows one
# (Scripts/python.exe, Scripts/maturin.exe) that the windows-latest leg needs.
# Without this, class A's defect could return with the self-test still green.
expect_venv_layouts() {
  local base="$SMOKE_TMP/venv-layout" posix="$SMOKE_TMP/venv-layout/posix" win="$SMOKE_TMP/venv-layout/windows"
  mkdir -p "$posix/bin" "$win/Scripts" "$base/empty" ||
    fail "self-test: cannot create the venv layout fixtures"
  : > "$posix/bin/python" && chmod +x "$posix/bin/python" || fail "self-test: cannot plant bin/python"
  : > "$posix/bin/maturin" && chmod +x "$posix/bin/maturin" || fail "self-test: cannot plant bin/maturin"
  : > "$win/Scripts/python.exe" || fail "self-test: cannot plant Scripts/python.exe"
  : > "$win/Scripts/maturin.exe" || fail "self-test: cannot plant Scripts/maturin.exe"
  expect_resolves_to "venv interpreter (POSIX layout)" "$posix/bin/python" venv_python "$posix"
  expect_resolves_to "venv interpreter (Windows layout)" "$win/Scripts/python.exe" venv_python "$win"
  expect_resolves_to "venv console script (POSIX layout)" "$posix/bin/maturin" venv_script "$posix" maturin
  expect_resolves_to "venv console script (Windows layout)" "$win/Scripts/maturin.exe" venv_script "$win" maturin
  expect_resolves_nothing "venv with neither layout" venv_python "$base/empty"
  expect_resolves_nothing "console script in neither layout" venv_script "$base/empty" maturin
}

# self_test — RELEASE.md's negative control for this proof: every assertion the
# install-and-assert half makes must be seen rejecting a tampered wheel (and
# accepting a well-formed one) before the default mode's ok line is earned.
# Hermetic throughout: planting, venv creation and --no-index installs only.
self_test() { # <ver> <name>
  local ver="$1" name="$2" p="$SMOKE_TMP/plant" cv="$SMOKE_TMP/import-venv"
  expect_venv_layouts
  plant_wheel "$p/good" "$ver" "$ver" "$PY_TAG" "$ABI_TAG" ||
    fail "self-test: cannot plant the well-formed wheel"
  plant_wheel "$p/metadata" "$ver" "$STALE_VER" "$PY_TAG" "$ABI_TAG" ||
    fail "self-test: cannot plant the metadata-version-mismatch wheel"
  plant_wheel "$p/pytag" "$ver" "$ver" cp39 "$ABI_TAG" ||
    fail "self-test: cannot plant the python-tag-mismatch wheel"
  plant_wheel "$p/wheel-tag" "$ver" "$ver" "$PY_TAG" "$ABI_TAG" "cp39-$ABI_TAG" ||
    fail "self-test: cannot plant the wheel-tag-mismatch wheel"
  expect_file_ok "well-formed wheel" "$p/good/"*.whl "$ver" "$name"
  expect_file_reject "metadata version mismatch" "$p/metadata/"*.whl "$ver" "$name" "METADATA version '$STALE_VER'"
  expect_file_reject "python tag mismatch" "$p/pytag/"*.whl "$ver" "$name" "python tag 'cp39'"
  expect_file_reject "wheel metadata tag mismatch" "$p/wheel-tag/"*.whl "$ver" "$name" "no 'Tag: ${PY_TAG}-${ABI_TAG}-'"
  make_venv "$cv" || fail "self-test: cannot create the import-control virtualenv"
  pip_install_offline "$VENV_PY" "$p/good/"*.whl ||
    fail "self-test: the well-formed wheel refused to install offline"
  expect_import_ok "well-formed wheel imports at the manifest version" "$VENV_PY" "$ver"
  pip_install_offline "$VENV_PY" "$p/metadata/"*.whl --force-reinstall ||
    fail "self-test: the tampered wheel refused to install offline"
  expect_import_reject "import version assertion fires on the tampered wheel" "$VENV_PY" "$ver" "reports version '$STALE_VER'"
  echo "self-test: ok"
}

cleanup() {
  rm -rf "${SMOKE_TMP:-}"
  if [ "$LOCK_GENERATED" -eq 1 ]; then
    rm -f "$ROOT/Cargo.lock"
  fi
  return 0
}

main() {
  local mode="build" wheel_arg="" ver name
  while [ $# -gt 0 ]; do
    case "$1" in
      --wheel)
        [ $# -ge 2 ] || { usage >&2; fail "--wheel needs a path"; }
        mode="wheel"
        wheel_arg="$2"
        shift 2
        ;;
      --self-test)
        mode="self-test"
        shift
        ;;
      -h | --help)
        usage
        exit 0
        ;;
      *)
        usage >&2
        fail "unknown argument '$1'"
        ;;
    esac
  done

  resolve_py
  MANIFEST="$ROOT/pulsehive-py/Cargo.toml"
  PYPROJECT="$ROOT/pulsehive-py/pyproject.toml"
  [ -f "$MANIFEST" ] || fail "missing $MANIFEST"
  [ -f "$PYPROJECT" ] || fail "missing $PYPROJECT"
  ver=$(manifest_version) || exit 1
  name=$(project_name) || exit 1
  check_py_version
  SMOKE_TMP=$(mktemp -d "${TMPDIR:-/tmp}/py-wheel-smoke.XXXXXX") || fail "cannot create a temp dir"
  trap cleanup EXIT

  case "$mode" in
    build)
      TARGET_DIR="${PULSEHIVE_WHEEL_SMOKE_TARGET_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/pulsehive/py-wheel-smoke/target}"
      mkdir -p "$TARGET_DIR" || fail "cannot create the cargo target dir '$TARGET_DIR'"
      [ -e "$ROOT/Cargo.lock" ] || LOCK_GENERATED=1
      local wheel
      wheel=$(build_wheel "$SMOKE_TMP/dist") || exit 1
      install_and_import "$wheel" "$ver" "$name" "$SMOKE_TMP" || exit 1
      echo "py wheel smoke: ok"
      ;;
    wheel)
      [ -f "$wheel_arg" ] || fail "--wheel: no such wheel: '$wheel_arg'"
      install_and_import "$wheel_arg" "$ver" "$name" "$SMOKE_TMP" || exit 1
      echo "py wheel smoke: ok"
      ;;
    self-test)
      self_test "$ver" "$name"
      ;;
  esac
}

ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
main "$@"
