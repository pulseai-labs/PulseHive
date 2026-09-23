#!/usr/bin/env bash
#
# pack-npm.sh — the single npm packaging path for @pulsehive/sdk.
#
# CI (.github/workflows/npm-release.yml) and a developer's machine run this same
# script, so the tarballs that reach the registry are the ones that were checked
# against the advertised targets. Nothing here is committed: the `npm/` tree is
# generated at pack time (see pulsehive-js/.gitignore).
#
# Usage
#   pack-npm.sh --artifacts <dir> [--targets all|host] [--out <dir>]
#   pack-npm.sh --self-test
#
#   --artifacts <dir>   Directory holding the built `pulsehive-js.<platform>.node`
#                       files. They may sit flat in it, or one level deep under a
#                       per-artifact directory, which is the layout
#                       actions/download-artifact produces when `merge-multiple`
#                       is left off. Both are searched.
#   --targets all|host  Which targets must be present and get packaged.
#                         all  — every target declared in package.json (default)
#                         host — only this machine's target
#   --out <dir>         Where the generated .tgz files land.
#                       Default: <pulsehive-js>/dist-npm
#   --self-test         Exercise the artifact/target check against throwaway
#                       fixtures and print `self-test: ok`. Runs a fixed set of
#                       cases — one missing artifact, one unexpected artifact,
#                       one mislabelled artifact, and one fully valid set — and
#                       never touches the real npm/ directory or package.json.
#
# The check fails closed, naming the target, when an expected artifact is
# missing, when an unexpected `.node` file is present, or when an artifact's
# binary format does not match the target it claims to be.
#
# Requires: bash, node + npm, and file(1).
#
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
PKG_DIR="$(cd -- "$SCRIPT_DIR/.." && pwd -P)"
PKG_JSON="$PKG_DIR/package.json"

DEFAULT_OUT="$PKG_DIR/dist-npm"

# --------------------------------------------------------------- reporting --

say() { printf '%s\n' "$*"; }
warn() { printf 'pack-npm: warning: %s\n' "$*" >&2; }
die() {
  printf 'pack-npm: error: %s\n' "$*" >&2
  exit 1
}

usage() {
  sed -n '3,34p' "${BASH_SOURCE[0]}" | sed 's/^#\{0,1\} \{0,1\}//'
}

# ------------------------------------------- target <-> napi platform maps --
#
# napi names its platform packages after the platform/arch/abi triple rather than
# the Rust target, so every target has to be translated. An unknown target is a
# hard error: guessing would silently mislabel an artifact.

platform_for_target() {
  case "$1" in
    aarch64-apple-darwin) echo "darwin-arm64" ;;
    x86_64-apple-darwin) echo "darwin-x64" ;;
    x86_64-unknown-linux-gnu) echo "linux-x64-gnu" ;;
    aarch64-unknown-linux-gnu) echo "linux-arm64-gnu" ;;
    x86_64-unknown-linux-musl) echo "linux-x64-musl" ;;
    aarch64-unknown-linux-musl) echo "linux-arm64-musl" ;;
    armv7-unknown-linux-gnueabihf) echo "linux-arm-gnueabihf" ;;
    x86_64-pc-windows-msvc) echo "win32-x64-msvc" ;;
    aarch64-pc-windows-msvc) echo "win32-arm64-msvc" ;;
    *) return 1 ;;
  esac
}

# The format `file` must report for a platform package. Matching is done on the
# lower-cased output and only on the two tokens that identify a binary: its
# container family and its architecture. Wording differs between file(1)
# releases ("PE32+ executable (DLL) (console) x86-64, for MS Windows" against
# "PE32+ executable for MS Windows 6.00 (DLL), x86-64"), so matching the whole
# sentence would be brittle in exactly the direction that breaks CI.
format_re_for_platform() {
  case "$1" in
    darwin-arm64) echo 'mach-o.*(arm64|aarch64)' ;;
    darwin-x64) echo 'mach-o.*x86[_-]64' ;;
    linux-x64-gnu | linux-x64-musl) echo 'elf.*x86[_-]64' ;;
    linux-arm64-gnu | linux-arm64-musl) echo 'elf.*aarch64' ;;
    linux-arm-gnueabihf) echo 'elf.*arm' ;;
    win32-x64-msvc) echo 'pe32[+].*x86[_-]64' ;;
    win32-arm64-msvc) echo 'pe32[+].*(aarch64|arm64)' ;;
    *) return 1 ;;
  esac
}

# ------------------------------------------------------------- requirements --

require_cmd() {
  command -v "$1" >/dev/null 2>&1 || die "required tool '$1' is not on PATH"
}

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

# ------------------------------------------------------------ host triple ----

detect_host_triple() {
  local host
  if command -v rustc >/dev/null 2>&1; then
    host="$(rustc -vV 2>/dev/null | sed -n 's/^host: //p')"
    if [ -n "$host" ]; then
      printf '%s\n' "$host"
      return 0
    fi
  fi
  case "$(uname -s)/$(uname -m)" in
    Linux/x86_64) echo x86_64-unknown-linux-gnu ;;
    Linux/aarch64 | Linux/arm64) echo aarch64-unknown-linux-gnu ;;
    Darwin/arm64) echo aarch64-apple-darwin ;;
    Darwin/x86_64) echo x86_64-apple-darwin ;;
    MINGW*/x86_64 | MSYS*/x86_64 | CYGWIN*/x86_64) echo x86_64-pc-windows-msvc ;;
    MINGW*/aarch64 | MSYS*/aarch64 | CYGWIN*/aarch64) echo aarch64-pc-windows-msvc ;;
  esac
}

# ------------------------------------------------------- artifact checking ---
#
# check_artifacts <artifacts-dir> <binary-name> <target>...
#
# Returns 0 only when the directory holds exactly one artifact per named target,
# each in the format that target implies. On failure it prints every problem it
# found to stdout, names the offending target, and returns 1 — it never exits, so
# --self-test can assert on the rejection.

# Every `.node` file under the artifacts directory, keyed by basename, printed as
# "<basename>\t<absolute path>". Both the flat layout (--artifacts /tmp/art) and
# the nested download-artifact layout are covered.
_list_artifacts() {
  local dir="$1" f
  while IFS= read -r f; do
    [ -n "$f" ] || continue
    printf '%s\t%s\n' "${f##*/}" "$f"
  done < <(find "$dir" -type f -name '*.node' | sort)
}

check_artifacts() {
  local dir="$1" bin="$2"
  shift 2
  local targets=("$@")

  local -a problems=()
  local listed target platform expected found_path dup

  listed="$(_list_artifacts "$dir")"

  # A basename that appears twice cannot be resolved to one file, and letting
  # `napi artifacts` pick arbitrarily is how a stale binary ships.
  dup="$(printf '%s\n' "$listed" | cut -f1 | sort | uniq -d)"
  if [ -n "$dup" ]; then
    while IFS= read -r d; do
      [ -n "$d" ] || continue
      problems+=("duplicate artifact '$d' appears more than once under $dir")
    done <<<"$dup"
  fi

  # 1. Every expected target must have its artifact.
  local -a expected_names=()
  for target in "${targets[@]}"; do
    if ! platform="$(platform_for_target "$target")"; then
      problems+=("target '$target' has no known napi platform package name")
      continue
    fi
    expected="$bin.$platform.node"
    expected_names+=("$expected")
    if ! printf '%s\n' "$listed" | cut -f1 | grep -qxF "$expected"; then
      problems+=("missing artifact for target $target (expected $expected)")
    fi
  done

  # 2. Nothing else may be present.
  local name
  while IFS=$'\t' read -r name found_path; do
    [ -n "$name" ] || continue
    local known=0 e
    for e in "${expected_names[@]}"; do
      [ "$name" = "$e" ] && known=1 && break
    done
    if [ "$known" -eq 0 ]; then
      problems+=("unexpected artifact '$name' is not produced by any expected target ($found_path)")
    fi
  done <<<"$listed"

  # 3. Each artifact must actually be the format its name claims.
  local reported re
  for target in "${targets[@]}"; do
    platform="$(platform_for_target "$target")" || continue
    expected="$bin.$platform.node"
    found_path="$(printf '%s\n' "$listed" | awk -F'\t' -v n="$expected" '$1 == n { print $2; exit }')"
    [ -n "$found_path" ] || continue
    if ! re="$(format_re_for_platform "$platform")"; then
      problems+=("no known binary format for target $target ($platform)")
      continue
    fi
    reported="$(file -b "$found_path" 2>/dev/null | tr '[:upper:]' '[:lower:]')"
    if ! printf '%s' "$reported" | grep -Eq -- "$re"; then
      problems+=("mislabelled artifact for target $target: $expected is '$reported', which is not the expected format")
    fi
  done

  if [ "${#problems[@]}" -gt 0 ]; then
    local p
    for p in "${problems[@]}"; do
      printf 'pack-npm: %s\n' "$p"
    done
    return 1
  fi
  return 0
}

# ------------------------------------------------------------- self-test -----
#
# Fixtures are throwaway files with fake names, built in a temp dir; the real
# npm/ tree and package.json are never read or written here. The valid fixtures
# carry a minimal but genuine binary header for their target, because an empty
# file cannot tell "this artifact is mislabelled" apart from "this artifact is
# empty" — and the check's whole job is the former.

_fixture_write() { # <path> <format>
  local path="$1" fmt="$2"
  FIXTURE_FMT="$fmt" FIXTURE_OUT="$path" node -e '
    const fs = require("fs");
    const fmt = process.env.FIXTURE_FMT;
    let buf;
    if (fmt === "elf-x86-64") {
      buf = Buffer.alloc(64);
      buf.write("\x7fELF", 0, "binary");
      buf[4] = 2; buf[5] = 1; buf[6] = 1;      // 64-bit, little endian
      buf.writeUInt16LE(3, 16);                 // ET_DYN
      buf.writeUInt16LE(0x3e, 18);              // EM_X86_64
      buf.writeUInt32LE(1, 20);
      buf.writeUInt16LE(64, 40); buf.writeUInt16LE(56, 42); buf.writeUInt16LE(64, 46);
    } else if (fmt === "macho-arm64") {
      buf = Buffer.alloc(32);
      buf.writeUInt32LE(0xfeedfacf, 0);         // MH_MAGIC_64
      buf.writeUInt32LE(0x0100000c, 4);         // CPU_TYPE_ARM64
      buf.writeUInt32LE(0, 8);
      buf.writeUInt32LE(2, 12);                 // MH_EXECUTE
    } else if (fmt === "pe32plus-x86-64") {
      buf = Buffer.alloc(0x200);
      buf.write("MZ", 0, "ascii");
      buf.writeUInt32LE(0x40, 0x3c);            // e_lfanew
      buf.write("PE\0\0", 0x40, "binary");
      buf.writeUInt16LE(0x8664, 0x44);          // IMAGE_FILE_MACHINE_AMD64
      buf.writeUInt16LE(1, 0x46);
      buf.writeUInt16LE(0xf0, 0x54);
      buf.writeUInt16LE(0x2022, 0x56);
      buf.writeUInt16LE(0x20b, 0x58);           // PE32+ optional header magic
      buf[0x5a] = 14;
      buf.writeUInt32LE(0x1000, 0x58 + 0x04);
      buf.writeUInt32LE(0x1000, 0x58 + 0x10);
      buf.writeBigUInt64LE(0x180000000n, 0x58 + 0x18);
      buf.writeUInt32LE(0x1000, 0x58 + 0x20);
      buf.writeUInt32LE(0x200, 0x58 + 0x24);
      buf.writeUInt16LE(6, 0x58 + 0x28);
      buf.writeUInt16LE(6, 0x58 + 0x30);
      buf.writeUInt32LE(0x2000, 0x58 + 0x38);
      buf.writeUInt32LE(0x400, 0x58 + 0x3c);
      buf.writeUInt16LE(2, 0x58 + 0x44);        // IMAGE_SUBSYSTEM_WINDOWS_GUI
      buf.writeUInt16LE(0x160, 0x58 + 0x46);
      buf.writeBigUInt64LE(0x100000n, 0x58 + 0x48);
      buf.writeBigUInt64LE(0x1000n, 0x58 + 0x50);
      buf.writeBigUInt64LE(0x100000n, 0x58 + 0x58);
      buf.writeBigUInt64LE(0x1000n, 0x58 + 0x60);
      buf.writeUInt32LE(16, 0x58 + 0x6c);
    } else {
      console.error("unknown fixture format " + fmt);
      process.exit(1);
    }
    fs.writeFileSync(process.env.FIXTURE_OUT, buf);
  '
}

_selftest_fail() {
  printf 'self-test: FAIL — %s\n' "$1"
}

# Expect check_artifacts to reject, and to say why by naming $needle.
_selftest_reject() { # <label> <dir> <needle> <target>...
  local label="$1" dir="$2" needle="$3"
  shift 3
  local out
  if out="$(check_artifacts "$dir" "$SELFTEST_BIN" "$@" 2>&1)"; then
    _selftest_fail "$label: the check accepted an artifact set it must reject"
    return 1
  fi
  if ! printf '%s' "$out" | grep -qF -- "$needle"; then
    _selftest_fail "$label: rejected, but the reason does not name '$needle' (got: $out)"
    return 1
  fi
  return 0
}

# Expect check_artifacts to accept.
_selftest_accept() { # <label> <dir> <target>...
  local label="$1" dir="$2"
  shift 2
  local out
  if ! out="$(check_artifacts "$dir" "$SELFTEST_BIN" "$@" 2>&1)"; then
    _selftest_fail "$label: the check rejected a valid artifact set ($out)"
    return 1
  fi
  return 0
}

# The self-test builds its fixtures in a temp dir; an EXIT trap is what keeps the
# cleanup process-scoped, since a RETURN trap set inside a function leaks into
# every later function return.
SELFTEST_TMP=""
selftest_cleanup() {
  [ -n "$SELFTEST_TMP" ] || return 0
  rm -rf -- "$SELFTEST_TMP"
  SELFTEST_TMP=""
}

run_self_test() {
  local targets
  targets="$(pkg_get napi.targets)"
  if [ -z "$targets" ]; then
    printf 'self-test: FAIL — package.json declares no napi.targets to check against\n'
    return 1
  fi

  local -a target_list=()
  while IFS= read -r t; do
    [ -n "$t" ] || continue
    target_list+=("$t")
  done <<<"$targets"

  # A name that is deliberately not the real binaryName: the check must follow
  # the name it is handed, never a hardcoded one.
  SELFTEST_BIN="selftest-bin"

  local tmp rc=0 d p
  tmp="$(mktemp -d)"
  SELFTEST_TMP="$tmp"
  trap selftest_cleanup EXIT

  local -a platforms=() names=()
  local t
  for t in "${target_list[@]}"; do
    if ! p="$(platform_for_target "$t")"; then
      printf 'self-test: FAIL — target %s has no napi platform mapping\n' "$t"
      return 1
    fi
    platforms+=("$p")
    names+=("$SELFTEST_BIN.$p.node")
  done

  # (a) one expected artifact missing.
  d="$tmp/missing"
  mkdir -p "$d"
  local i
  for i in "${!names[@]}"; do
    if [ "$i" -eq 0 ]; then
      continue                          # drop the first expected artifact
    fi
    : >"$d/${names[$i]}"
  done
  _selftest_reject "missing artifact" "$d" "missing artifact for target ${target_list[0]}" "${target_list[@]}" || rc=1

  # (b) one unexpected .node present alongside a complete set.
  d="$tmp/unexpected"
  mkdir -p "$d"
  for i in "${!names[@]}"; do
    : >"$d/${names[$i]}"
  done
  : >"$d/$SELFTEST_BIN.freebsd-x64.node"
  _selftest_reject "unexpected artifact" "$d" "unexpected artifact '$SELFTEST_BIN.freebsd-x64.node'" "${target_list[@]}" || rc=1

  # (c) an artifact whose binary format does not match its name: every expected
  #     artifact is present and correctly formatted except the last one, which
  #     carries another target's format. Rejection must single that one out.
  d="$tmp/mislabelled"
  mkdir -p "$d"
  for i in "${!names[@]}"; do
    case "${platforms[$i]}" in
      linux-x64-gnu | linux-x64-musl) _fixture_write "$d/${names[$i]}" elf-x86-64 ;;
      darwin-arm64) _fixture_write "$d/${names[$i]}" macho-arm64 ;;
      *) _fixture_write "$d/${names[$i]}" pe32plus-x86-64 ;;
    esac
  done
  i=$(( ${#names[@]} - 1 ))
  if [ "${platforms[$i]}" = "linux-x64-gnu" ] || [ "${platforms[$i]}" = "linux-x64-musl" ]; then
    _fixture_write "$d/${names[$i]}" macho-arm64
  else
    _fixture_write "$d/${names[$i]}" elf-x86-64
  fi
  _selftest_reject "mislabelled artifact" "$d" "mislabelled artifact for target ${target_list[$i]}" "${target_list[@]}" || rc=1

  # (d) the same set, correctly labelled, must be accepted. Without this a check
  #     that rejected everything would pass (a)-(c) and still look healthy.
  d="$tmp/valid"
  mkdir -p "$d"
  for i in "${!names[@]}"; do
    case "${platforms[$i]}" in
      linux-x64-gnu | linux-x64-musl) _fixture_write "$d/${names[$i]}" elf-x86-64 ;;
      darwin-arm64) _fixture_write "$d/${names[$i]}" macho-arm64 ;;
      win32-x64-msvc) _fixture_write "$d/${names[$i]}" pe32plus-x86-64 ;;
      *)
        printf 'self-test: FAIL — no synthetic fixture format for platform %s\n' "${platforms[$i]}"
        rc=1
        ;;
    esac
  done
  if [ "$rc" -eq 0 ]; then
    _selftest_accept "valid set" "$d" "${target_list[@]}" || rc=1
  fi

  if [ "$rc" -ne 0 ]; then
    return "$rc"
  fi
  say "self-test: ok"
  return 0
}

# ------------------------------------------------------------------ packing --

napi() {
  ( cd -- "$PKG_DIR" && npx --no-install napi "$@" )
}

# The generated loader has to be at the package root before the main package is
# packed. `files` advertises both index.js and index.d.ts, and wrapper.js requires
# the former, but both are napi-generated and gitignored — so a tree that only
# downloaded artifacts (the `pack` job, which never runs `napi build`) has
# neither, and an `npm pack` of such a tree exits 0 while silently omitting a
# missing `files` entry. `napi artifacts` restores index.js from --artifacts but
# not index.d.ts, so the guarantee is made here, explicitly, for both files: take
# it from the package root when it is already there — the local `--targets host`
# path, where a built tree has both — otherwise from the recursive search under
# --artifacts. When it is in neither, fail closed: `npm pack` would exit 0 and ship
# a main package missing an advertised entry, so the load-bearing behaviour here is
# the refusal, not the copy. Idempotent on the paths where both are already present.
ensure_generated_loader() {
  local name src m
  local -a missing=()
  for name in index.js index.d.ts; do
    if [ -f "$PKG_DIR/$name" ]; then
      continue
    fi
    src="$(find "$ARTIFACTS_DIR" -type f -name "$name" | sort | head -n1)"
    if [ -n "$src" ]; then
      cp -- "$src" "$PKG_DIR/$name"
      say "pack-npm: placed generated $name from $src"
    else
      missing+=("$name")
    fi
  done
  if [ "${#missing[@]}" -gt 0 ]; then
    for m in "${missing[@]}"; do
      printf 'pack-npm: missing generated %s (not at %s and not under %s)\n' \
        "$m" "$PKG_DIR" "$ARTIFACTS_DIR" >&2
    done
    die "the main package would omit advertised 'files' entries; refusing to pack"
  fi
}

sync_npm_package_versions() {
  local version="$1" f
  for f in "$PKG_DIR"/npm/*/package.json; do
    [ -e "$f" ] || continue
    node -e '
      const fs = require("fs");
      const [file, version] = process.argv.slice(1);
      const pkg = JSON.parse(fs.readFileSync(file, "utf8"));
      if (pkg.version !== version) {
        pkg.version = version;
        fs.writeFileSync(file, JSON.stringify(pkg, null, 2) + "\n");
      }
    ' "$f" "$version"
  done
}

pack_dir() { # <dir> — prints the absolute path of the tarball it produced
  local dir="$1" name
  if ! name="$( cd -- "$dir" && npm pack --pack-destination "$OUT_DIR" 2>/dev/null | tail -n1 )"; then
    die "npm pack failed in $dir"
  fi
  [ -n "$name" ] || die "npm pack produced no tarball in $dir"
  printf '%s/%s\n' "$OUT_DIR" "$name"
}

# --------------------------------------------------------------------- main --

ARTIFACTS_DIR=""
TARGET_MODE="all"
OUT_DIR=""
SELF_TEST=0

while [ "$#" -gt 0 ]; do
  case "$1" in
    --artifacts)
      [ "$#" -ge 2 ] || die "--artifacts needs a directory"
      ARTIFACTS_DIR="$2"
      shift 2
      ;;
    --targets)
      [ "$#" -ge 2 ] || die "--targets needs 'all' or 'host'"
      TARGET_MODE="$2"
      shift 2
      ;;
    --out)
      [ "$#" -ge 2 ] || die "--out needs a directory"
      OUT_DIR="$2"
      shift 2
      ;;
    --self-test)
      SELF_TEST=1
      shift
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      printf 'pack-npm: error: unknown argument %s\n\n' "$1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

case "$TARGET_MODE" in
  all | host) ;;
  *) die "--targets takes 'all' or 'host', got '$TARGET_MODE'" ;;
esac

if [ "$SELF_TEST" -eq 1 ]; then
  require_cmd node
  require_cmd file
  run_self_test
  exit $?
fi

[ -n "$ARTIFACTS_DIR" ] || {
  printf 'pack-npm: error: --artifacts <dir> is required (or use --self-test)\n\n' >&2
  usage >&2
  exit 2
}

require_cmd node
require_cmd npm
require_cmd file
[ -f "$PKG_JSON" ] || die "package.json not found at $PKG_JSON"

BIN_NAME="$(pkg_get napi.binaryName)"
[ -n "$BIN_NAME" ] || die "package.json has no napi.binaryName"
MAIN_NAME="$(pkg_get name)"
[ -n "$MAIN_NAME" ] || die "package.json has no name"
VERSION="$(pkg_get version)"
[ -n "$VERSION" ] || die "package.json has no version"

declared="$(pkg_get napi.targets)"
[ -n "$declared" ] || die "package.json declares no napi.targets"
declare -a DECLARED=()
while IFS= read -r t; do
  [ -n "$t" ] || continue
  DECLARED+=("$t")
done <<<"$declared"

declare -a EXPECTED=()
case "$TARGET_MODE" in
  all) EXPECTED=("${DECLARED[@]}") ;;
  host)
    host_triple="$(detect_host_triple)"
    [ -n "$host_triple" ] || die "cannot determine this machine's Rust host triple; use --targets all"
    EXPECTED=("$host_triple")
    ;;
esac

# Every expected target must be one the package advertises.
for t in "${EXPECTED[@]}"; do
  advertised=0
  for d in "${DECLARED[@]}"; do
    [ "$t" = "$d" ] && advertised=1 && break
  done
  [ "$advertised" -eq 1 ] || die "target '$t' is not declared in package.json napi.targets"
done

[ -d "$ARTIFACTS_DIR" ] || die "artifacts directory '$ARTIFACTS_DIR' does not exist"
ARTIFACTS_DIR="$(cd -- "$ARTIFACTS_DIR" && pwd -P)"
if [ -z "$OUT_DIR" ]; then
  OUT_DIR="$DEFAULT_OUT"
fi
mkdir -p "$OUT_DIR"
OUT_DIR="$(cd -- "$OUT_DIR" && pwd -P)"

# Fail closed before anything is generated.
if ! check_out="$(check_artifacts "$ARTIFACTS_DIR" "$BIN_NAME" "${EXPECTED[@]}")"; then
  printf '%s\n' "$check_out" >&2
  die "artifact check failed for ${#EXPECTED[@]} target(s); refusing to pack"
fi

# `napi` insists on artifacts for every configured target, so when only a subset
# is being packed it is handed a config that narrows napi.targets. The tracked
# package.json is not the vehicle for that.
NAPI_CONFIG=""
if [ "${#EXPECTED[@]}" -ne "${#DECLARED[@]}" ]; then
  NAPI_CONFIG="$(mktemp)"
  node -e '
    const fs = require("fs");
    const targets = process.argv.slice(2);
    fs.writeFileSync(process.argv[1], JSON.stringify({ targets }, null, 2) + "\n");
  ' "$NAPI_CONFIG" "${EXPECTED[@]}"
fi

CONFIG_ARGS=()
if [ -n "$NAPI_CONFIG" ]; then
  CONFIG_ARGS=(--config-path "$NAPI_CONFIG")
fi

PKG_BACKUP=""
cleanup() {
  if [ -n "$PKG_BACKUP" ] && [ -f "$PKG_BACKUP" ]; then
    cp -- "$PKG_BACKUP" "$PKG_JSON"
    rm -f -- "$PKG_BACKUP"
  fi
  if [ -n "$NAPI_CONFIG" ] && [ -f "$NAPI_CONFIG" ]; then
    rm -f -- "$NAPI_CONFIG"
  fi
}
trap cleanup EXIT

# The npm/ tree is generated at pack time and never committed.
rm -rf -- "$PKG_DIR/npm"
NPM_DIR="$PKG_DIR/npm"

say "pack-npm: targets ${EXPECTED[*]}"
napi create-npm-dirs --npm-dir "$NPM_DIR" "${CONFIG_ARGS[@]+"${CONFIG_ARGS[@]}"}"
napi artifacts --output-dir "$ARTIFACTS_DIR" --npm-dir "$NPM_DIR" "${CONFIG_ARGS[@]+"${CONFIG_ARGS[@]}"}"
sync_npm_package_versions "$VERSION"

# napi pre-publish writes the exact-version optionalDependencies into
# package.json. Those belong to the packed tarball only, so the tracked file is
# saved first and restored once packing ends — on failure as well as success.
PKG_BACKUP="$(mktemp)"
cp -- "$PKG_JSON" "$PKG_BACKUP"
napi pre-publish -t npm --skip-optional-publish --npm-dir "$NPM_DIR" "${CONFIG_ARGS[@]+"${CONFIG_ARGS[@]}"}"

# Platform packages first: the main package's optionalDependencies point at
# them, so that is also the order they have to be published in.
for t in "${EXPECTED[@]}"; do
  p="$(platform_for_target "$t")" || die "target '$t' has no napi platform package name"
  [ -d "$NPM_DIR/$p" ] || die "expected platform package directory $NPM_DIR/$p was not created"
  pack_dir "$NPM_DIR/$p"
done
# The main package carries the generated loader; platform packages do not, which
# is why this runs once, immediately before the main package is packed.
ensure_generated_loader
pack_dir "$PKG_DIR"

cleanup
PKG_BACKUP=""
trap - EXIT
say "pack-npm: done (${#EXPECTED[@]} platform package(s) + $MAIN_NAME $VERSION)"
