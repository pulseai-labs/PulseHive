#!/usr/bin/env bash
#
# npm-local-registry-smoke.sh — prove that the tarballs pack-npm.sh produces
# install the way the public npm registry will serve them.
#
# The proof, end to end:
#   1. npm ci, then a host debug build of the native addon;
#   2. pack-npm.sh --targets host, which produces the main tarball plus the
#      host's platform tarball;
#   3. a throwaway verdaccio on a free localhost port, with in-scratch storage,
#      no uplink, and anonymous publish of @pulsehive/* only;
#   4. publish the platform tarball first, then the main tarball;
#   5. a fresh scratch consumer installs @pulsehive/sdk from that registry —
#      the only path that exercises optionalDependencies platform resolution
#      (#97 broke exactly this, and a bare `npm pack` install never reaches it);
#   6. that consumer requires the package, builds a HiveMind over a temp
#      substrate path, and prints `npm local-registry smoke: ok`.
#
# --negative runs the same steps with the host platform package deliberately NOT
# published: the install must still succeed and `require` must throw w2's
# unadvertised-host error (`does not support`). That control is what makes step
# 6's success mean something — without it, an install that silently fell back to
# the local source tree would print `ok` too.
#
# Nothing here is committed and nothing real is published: every byte of scratch
# (the tarballs, the registry's config and storage, the consumer project, the
# verdaccio install) lives under one mktemp -d removed on exit, and the run
# leaves the worktree's `git status --porcelain` exactly as it found it.
#
# Usage
#   npm-local-registry-smoke.sh [--negative]
#
# Requires: bash, node (>=22) + npm, cargo/rustc, file(1), git, and network
# access to the public npm registry for `npm ci` and the pinned verdaccio.
#
set -euo pipefail

# ---------------------------------------------------------------- locating --

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
PKG_DIR="$(cd -- "$SCRIPT_DIR/.." && pwd -P)"
REPO_ROOT="$(cd -- "$PKG_DIR/.." && pwd -P)"
PKG_JSON="$PKG_DIR/package.json"

# verdaccio is pinned: the proof of a published contract must not move because
# a registry release did. Bumping this is a deliberate edit.
VERDACCIO_VERSION="6.10.4"

# The release's ledger budget covers every auto line together (600 s); this line
# is the spine's recurring one, so its own ceiling is 180 s.
WALL_BUDGET_SECONDS=180

# 0 — publish the host platform package and the main package, install them, call
#      the binding.
# 1 — same, with the host platform package deliberately withheld: the install
#      must still succeed and require() must throw the A3 error.
NEGATIVE=0

# --------------------------------------------------------------- reporting --

say() { printf '%s\n' "$*"; }
warn() { printf 'npm-local-registry-smoke: warning: %s\n' "$*" >&2; }
# Per-step elapsed seconds, so a run that creeps toward the 180 s ceiling says
# which step spent the budget.
mark() { printf '    [%ss]\n' "$SECONDS"; }
die() {
  printf 'npm-local-registry-smoke: error: %s\n' "$*" >&2
  exit 1
}

usage() {
  printf '%s\n' \
    'Usage: npm-local-registry-smoke.sh [--negative]' \
    '' \
    '  (no flag)   package the host target, publish it and the main package to' \
    '              a throwaway local registry, install them into a fresh' \
    '              consumer, and call the binding' \
    '  --negative  same, with the host platform package withheld: the install' \
    '              must succeed and require() must throw the unadvertised-host' \
    '              error' \
    '' \
    '  -h, --help  this message'
}

# ------------------------------------------------------------- preconditions --

require_cmd() {
  command -v "$1" >/dev/null 2>&1 || die "required tool '$1' is not on PATH"
}

# Read a dotted path out of package.json. Arrays come back newline-separated.
pkg_get() { # <dotted path>
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

# A localhost port nothing is listening on. There is an unavoidable race between
# closing this socket and verdaccio binding the port; it is the OS picking the
# port that we want, not a reservation.
free_port() {
  node -e '
    const net = require("net");
    const s = net.createServer();
    s.unref();
    s.listen(0, "127.0.0.1", () => {
      process.stdout.write(String(s.address().port));
      s.close();
    });
  '
}

# ------------------------------------------------------------------ cleanup --

TMP=""
VERDACCIO_PID=""

cleanup() {
  local rc=$?
  trap - EXIT
  if [ -n "$VERDACCIO_PID" ] && kill -0 "$VERDACCIO_PID" 2>/dev/null; then
    kill "$VERDACCIO_PID" 2>/dev/null || true
    wait "$VERDACCIO_PID" 2>/dev/null || true
  fi
  if [ -n "$TMP" ] && [ -d "$TMP" ]; then
    rm -rf -- "$TMP"
  fi
  exit "$rc"
}

# --------------------------------------------------------------------- args --

while [ "$#" -gt 0 ]; do
  case "$1" in
    --negative)
      NEGATIVE=1
      shift
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      printf 'npm-local-registry-smoke: error: unknown argument %s\n\n' "$1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

# ---------------------------------------------------------------- preamble --

require_cmd node
require_cmd npm
require_cmd file
require_cmd git
require_cmd rustc

# `cargo`/`rustc` are not on a non-login PATH, and napi build needs them; the
# toolchain the addon is built with must be the one this shell resolves.
PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"
export PATH

git -C "$REPO_ROOT" rev-parse --git-dir >/dev/null 2>&1 ||
  die "$REPO_ROOT is not a git worktree; run this from the repository"

[ -f "$PKG_JSON" ] || die "package.json not found at $PKG_JSON"

# Whatever the worktree looked like when this started, it must look identical
# when it ends. Generated build output (index.js, *.node, npm/, node_modules/) is
# gitignored and so invisible here; anything that does show up is residue this
# run created, and that is a failure, not a footnote.
STATUS_BEFORE="$(git -C "$REPO_ROOT" status --porcelain)"

PKG_NAME="$(pkg_get name)"
VERSION="$(pkg_get version)"
BIN_NAME="$(pkg_get napi.binaryName)"
[ -n "$PKG_NAME" ] || die "package.json has no name"
[ -n "$VERSION" ] || die "package.json has no version"
[ -n "$BIN_NAME" ] || die "package.json has no napi.binaryName"

# npm's tarball filename for a package: the scope sigil and separator become
# dashes, so @pulsehive/sdk packs as pulsehive-sdk-<version>.tgz.
PKG_SLUG="$(printf '%s' "$PKG_NAME" | sed 's|^@||; s|/|-|g')"

ADVERTISED_TRIPLES="$(pkg_get napi.targets)"
[ -n "$ADVERTISED_TRIPLES" ] || die "package.json declares no napi.targets"

# ------------------------------------------------------------------ scratch --

TMP="$(mktemp -d)"
trap cleanup EXIT

ARTIFACTS_DIR="$TMP/artifacts"
OUT_DIR="$TMP/tarballs"
CONSUMER_DIR="$TMP/consumer"
REGISTRY_DIR="$TMP/registry"
REGISTRY_STORAGE="$REGISTRY_DIR/storage"
REGISTRY_CONFIG="$REGISTRY_DIR/config.yaml"
REGISTRY_LOG="$REGISTRY_DIR/verdaccio.log"
VERDACCIO_PREFIX="$TMP/verdaccio"
NPMRC="$TMP/npmrc"
PUBLISH_NPMRC="$TMP/npmrc-publish"
SUBSTRATE_PATH="$TMP/substrate.db"

mkdir -p -- "$ARTIFACTS_DIR" "$OUT_DIR" "$CONSUMER_DIR" "$REGISTRY_DIR" "$REGISTRY_STORAGE"

# --------------------------------------------------- 1-3: build, pack, start --

say "==> npm ci"
( cd -- "$PKG_DIR" && npm ci --no-audit --no-fund --loglevel=error )

say "==> napi build --platform --features napi (host debug)"
( cd -- "$PKG_DIR" && npx --no-install napi build --platform --features napi )

say "==> collect the host artifact and pack with pack-npm.sh --targets host"
# napi build leaves <binaryName>.<platform>.node next to package.json. Copy it
# flat into a scratch artifacts dir: pack-npm.sh refuses a set that is not
# exactly the requested targets, so a stale foreign artifact fails closed here
# with a message naming it rather than shipping a mislabelled binary.
copied=0
for artifact in "$PKG_DIR/$BIN_NAME".*.node; do
  [ -e "$artifact" ] || continue
  cp -- "$artifact" "$ARTIFACTS_DIR/"
  copied=$((copied + 1))
done
[ "$copied" -ge 1 ] || die "napi build produced no $BIN_NAME.*.node artifact in $PKG_DIR"

( cd -- "$PKG_DIR" &&
  bash scripts/pack-npm.sh --artifacts "$ARTIFACTS_DIR" --targets host --out "$OUT_DIR" )

MAIN_TARBALL="$OUT_DIR/$PKG_SLUG-$VERSION.tgz"
[ -f "$MAIN_TARBALL" ] || die "expected main tarball $MAIN_TARBALL was not produced"

# With --targets host there is exactly one platform tarball, and its filename
# carries the host's platform suffix — the same suffix npm resolves
# @pulsehive/sdk-<suffix> from. Taking it from the artifact rather than a second
# copy of pack-npm.sh's triple->platform map keeps the two in step by
# construction.
PLATFORM_TARBALL=""
for tarball in "$OUT_DIR/$PKG_SLUG"-*-"$VERSION".tgz; do
  [ -e "$tarball" ] || continue
  [ -n "$PLATFORM_TARBALL" ] && die "expected one platform tarball, found more than one in $OUT_DIR"
  PLATFORM_TARBALL="$tarball"
done
[ -n "$PLATFORM_TARBALL" ] || die "no platform tarball was produced in $OUT_DIR"

PLATFORM_BASE="${PLATFORM_TARBALL##*/}"
PLATFORM_BASE="${PLATFORM_BASE#"$PKG_SLUG"-}"
HOST_SUFFIX="${PLATFORM_BASE%-"$VERSION".tgz}"
[ -n "$HOST_SUFFIX" ] || die "cannot derive the host platform suffix from $PLATFORM_TARBALL"

# The platform package this host resolves, by the name npm will look for.
HOST_PLATFORM_PKG="$PKG_NAME-$HOST_SUFFIX"

say "    main:     ${MAIN_TARBALL##*/}"
say "    platform: ${PLATFORM_TARBALL##*/}  (host suffix: $HOST_SUFFIX)"
mark

# ------------------------------------------------------ 3: throwaway registry --

say "==> verdaccio $VERDACCIO_VERSION (scratch install, no uplink, anonymous publish)"
npm install --prefix "$VERDACCIO_PREFIX" --no-save --no-audit --no-fund \
  --no-package-lock --loglevel=error "verdaccio@$VERDACCIO_VERSION"
VERDACCIO_BIN="$VERDACCIO_PREFIX/node_modules/.bin/verdaccio"
[ -x "$VERDACCIO_BIN" ] || die "verdaccio did not install to $VERDACCIO_BIN"

PORT="$(free_port)"
[ -n "$PORT" ] || die "could not find a free localhost port"
REGISTRY="http://127.0.0.1:$PORT"

# No uplink at all: this registry can only ever serve what this run published,
# so a passing install cannot have been satisfied from the public registry.
#
# max_body_size is raised deliberately. verdaccio defaults the request-body cap
# to 10mb (build/api/endpoint/index.js: `config.max_body_size || "10mb"`), and a
# host *debug* addon is ~286mb of ELF, so the default rejects the platform
# tarball with 413 before it is ever stored.
{
  printf 'storage: %s\n' "$REGISTRY_STORAGE"
  printf 'listen: 127.0.0.1:%s\n' "$PORT"
  printf 'auth:\n  htpasswd:\n    file: %s\n    max_users: -1\n' "$REGISTRY_DIR/htpasswd"
  printf 'uplinks: {}\n'
  printf 'packages:\n'
  printf "  '@pulsehive/*':\n    access: \$all\n    publish: \$all\n"
  printf "  '**':\n    access: \$all\n    publish: \$none\n"
  printf 'max_body_size: 2gb\n'
  printf 'log: { type: stdout, format: pretty, level: warn }\n'
} >"$REGISTRY_CONFIG"

# Two scratch user configs, so the two halves of the proof stay separable.
#
# $NPMRC carries no credentials at all: the consumer install is anonymous, which
# is the property being proved. $PUBLISH_NPMRC adds only a placeholder token,
# because npm's client refuses to publish against a registry it holds no token
# for — it raises ENEEDAUTH before any request is sent, so verdaccio never gets
# the chance to answer (verified: no request reaches the registry log). With a
# token present the request is sent, and the placeholder is not a credential:
# verdaccio cannot verify it, falls back to the anonymous identity (`user:
# null`), and the server-side permission above — `$all` on @pulsehive/*, `$none`
# everywhere else — is what actually authorises the write. Neither file touches
# ~/.npmrc.
printf 'registry=%s/\n' "$REGISTRY" >"$NPMRC"
{
  printf 'registry=%s/\n' "$REGISTRY"
  printf '//127.0.0.1:%s/:_authToken=local-registry-smoke-placeholder\n' "$PORT"
} >"$PUBLISH_NPMRC"

node "$VERDACCIO_BIN" --config "$REGISTRY_CONFIG" --listen "127.0.0.1:$PORT" \
  >"$REGISTRY_LOG" 2>&1 &
VERDACCIO_PID=$!

ready=0
for _ in $(seq 1 120); do
  if ! kill -0 "$VERDACCIO_PID" 2>/dev/null; then
    printf 'npm-local-registry-smoke: error: verdaccio exited during startup; last log lines:\n' >&2
    tail -n 40 -- "$REGISTRY_LOG" >&2 || true
    die "verdaccio did not stay up"
  fi
  # Any HTTP answer proves the port is served; fetch() only rejects on a
  # connection error, so a 404 counts as ready.
  if node -e 'fetch(process.argv[1]).then(() => process.exit(0)).catch(() => process.exit(1))' \
    "$REGISTRY/-/ping" 2>/dev/null; then
    ready=1
    break
  fi
  sleep 0.5
done
[ "$ready" -eq 1 ] || {
  tail -n 40 -- "$REGISTRY_LOG" >&2 || true
  die "verdaccio did not answer on $REGISTRY within 60s"
}
say "    registry: $REGISTRY (storage $REGISTRY_STORAGE)"
mark

# ------------------------------------------------------------- 4: publish --

publish_tarball() { # <tarball>
  say "    publish ${1##*/}"
  npm publish "$1" --registry "$REGISTRY" --userconfig "$PUBLISH_NPMRC" --access public --loglevel=warn
}

if [ "$NEGATIVE" -eq 1 ]; then
  say "==> publish (negative control: only the main package — $HOST_PLATFORM_PKG withheld)"
  publish_tarball "$MAIN_TARBALL"
else
  say "==> publish (platform package first: the main package's optionalDependencies pin it)"
  publish_tarball "$PLATFORM_TARBALL"
  publish_tarball "$MAIN_TARBALL"
fi
mark

# ------------------------------------------------------- 4-5: fresh consumer --

say "==> install into a fresh consumer"
(
  cd -- "$CONSUMER_DIR" &&
    npm init -y >/dev/null &&
    npm install "$PKG_NAME@$VERSION" --registry "$REGISTRY" --userconfig "$NPMRC"
)

INSTALLED_MAIN="$CONSUMER_DIR/node_modules/$PKG_NAME"
[ -f "$INSTALLED_MAIN/package.json" ] || die "$PKG_NAME was not installed into the consumer"

if [ "$NEGATIVE" -eq 1 ]; then
  # ------------------------------------------------- the negative control --
  #
  # The control is only worth anything if the platform package really is
  # missing: if it were present, require() would succeed and prove nothing about
  # the unadvertised-host path.
  if [ -e "$CONSUMER_DIR/node_modules/$HOST_PLATFORM_PKG" ]; then
    die "$HOST_PLATFORM_PKG is installed although it was never published; the control is not isolated"
  fi
  say "    install succeeded with $HOST_PLATFORM_PKG never published"

  say "==> require must throw the unadvertised-host error"
  # From inside the consumer, as the positive path runs: the error has to come
  # out of the installed package, not out of the repository this script lives in.
  (
    cd -- "$CONSUMER_DIR" &&
      SMOKE_PKG_NAME="$PKG_NAME" SMOKE_ADVERTISED_TRIPLES="$ADVERTISED_TRIPLES" \
        node -e '
    const pkgName = process.env.SMOKE_PKG_NAME;
    let error = null;
    try {
      require(pkgName);
    } catch (e) {
      error = e;
    }
    if (!error) {
      throw new Error(`${pkgName} loaded although its platform package was never published`);
    }
    const message = String(error.message || "");
    // ADR-015 A3 / ADR-007: the consumer gets the host and the advertised
    // matrix, not a bare module-resolution trace.
    const host = `${process.platform}/${process.arch}`;
    if (!message.includes("does not support")) {
      throw new Error(`expected the unadvertised-host error ("does not support"), got: ${message}`);
    }
    if (!message.includes(host)) {
      throw new Error(`the error does not name this host (${host}): ${message}`);
    }
    const triples = (process.env.SMOKE_ADVERTISED_TRIPLES || "").split("\n").filter(Boolean);
    if (triples.length === 0) {
      throw new Error("no advertised targets were passed in to check the error against");
    }
    for (const triple of triples) {
      if (!message.includes(triple)) {
        throw new Error(`the error does not list advertised target ${triple}: ${message}`);
      }
    }
    console.log(`    require() threw: ${message}`);
    console.log(
      `negative control: rejected as expected (${host}; advertised: ${triples.join(", ")})`,
    );
  '
  )
else
  # What the published package declares it can resolve. This is the machinery #97
  # broke: the loader requires @pulsehive/sdk-<host>, and it is the main package's
  # optionalDependencies — pinned to its own exact version (ADR-015 A2) — that
  # makes npm fetch it. Printed because it is the evidence that resolution went
  # through that path and not through a stray local directory.
  DECLARED_OPTIONALS="$(
    node -e '
      const fs = require("fs");
      const pkg = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
      process.stdout.write(Object.keys(pkg.optionalDependencies || {}).join("\n"));
    ' "$INSTALLED_MAIN/package.json"
  )"
  [ -n "$DECLARED_OPTIONALS" ] || die "the published $PKG_NAME declares no optionalDependencies"
  say "    optionalDependencies: $(printf '%s' "$DECLARED_OPTIONALS" | tr '\n' ' ')"

  [ -d "$CONSUMER_DIR/node_modules/$HOST_PLATFORM_PKG" ] ||
    die "$HOST_PLATFORM_PKG was not installed — optionalDependencies resolution did not run"

  # The main package carries no binary of its own (ADR-015 A2); the addon lives in
  # the platform package and nowhere else.
  if [ -n "$(find "$INSTALLED_MAIN" -name '*.node' -print -quit)" ]; then
    die "the main package's own directory holds a .node file; binaries belong in the platform package"
  fi

  # Nothing else from this package may be installed. The assertion is written
  # against the consumer's node_modules rather than against the declared
  # optionalDependencies on purpose: with `--targets host` napi injects only the
  # host entry, so the declared list is one long, and "the other two are absent"
  # has to mean the two advertised platforms this host did not resolve — which is
  # exactly what a scan of the installed scope can see.
  INSTALLED_PLATFORM_PKGS=0
  for dir in "$CONSUMER_DIR/node_modules/$PKG_NAME"-*; do
    [ -d "$dir" ] || continue
    [ "$dir" = "$CONSUMER_DIR/node_modules/$HOST_PLATFORM_PKG" ] || {
      die "${dir##*/node_modules/} is installed on a $HOST_SUFFIX host; the advertised matrix resolves only $HOST_PLATFORM_PKG here"
    }
    INSTALLED_PLATFORM_PKGS=$((INSTALLED_PLATFORM_PKGS + 1))
  done
  [ "$INSTALLED_PLATFORM_PKGS" -eq 1 ] ||
    die "expected exactly one platform package beside $PKG_NAME, found $INSTALLED_PLATFORM_PKGS"

  ADVERTISED_COUNT="$(printf '%s\n' "$ADVERTISED_TRIPLES" | grep -c .)"
  say "    installed $PKG_NAME@$VERSION + $HOST_PLATFORM_PKG (1 of $ADVERTISED_COUNT advertised platform packages resolved; the others are absent)"
  mark

  say "==> call the binding"
  # Run from inside the consumer: the whole point is that resolution goes through
  # what the install put in *its* node_modules, not through anything reachable
  # from the repository this script lives in.
  (
    cd -- "$CONSUMER_DIR" &&
      SMOKE_SUBSTRATE="$SUBSTRATE_PATH" SMOKE_PKG_NAME="$PKG_NAME" \
        SMOKE_NODE_MODULES="$CONSUMER_DIR/node_modules" \
        node -e '
      const pkgName = process.env.SMOKE_PKG_NAME;
      const sdk = require(pkgName);
      if (typeof sdk.HiveMind?.builder !== "function") {
        throw new Error("require() returned no HiveMind.builder()");
      }
      // The point of installing through a registry is that what loaded is the
      // installed copy, not the development tree next to this repository.
      const resolved = require.resolve(pkgName);
      const expected = `${process.env.SMOKE_NODE_MODULES}/`;
      if (!resolved.startsWith(expected)) {
        throw new Error(`resolved ${resolved}, which is not under ${expected}`);
      }
      const hive = sdk.HiveMind.builder().substratePath(process.env.SMOKE_SUBSTRATE).build();
      hive.shutdown();
      // A WASI or other fallback binding would mean the addon never came from the
      // platform package; only the native one is the published path.
      if (sdk.__napiBindingTarget && sdk.__napiBindingTarget !== "native") {
        throw new Error(`loaded the ${sdk.__napiBindingTarget} binding, not the native addon`);
      }
      const version = typeof sdk.version === "function" ? sdk.version() : sdk.version;
      console.log(
        `npm local-registry smoke: ok (${pkgName} ${version ?? "?"}, ` +
          `binding ${sdk.__napiBindingTarget ?? "?"}, ${resolved})`,
      );
    '
  )
fi
mark

# ------------------------------------------------------- worktree unchanged --

STATUS_AFTER="$(git -C "$REPO_ROOT" status --porcelain)"
if [ "$STATUS_AFTER" != "$STATUS_BEFORE" ]; then
  printf 'npm-local-registry-smoke: error: the run changed the worktree status\n' >&2
  printf '  before: %s\n' "${STATUS_BEFORE:-<clean>}" >&2
  printf '  after:  %s\n' "${STATUS_AFTER:-<clean>}" >&2
  die "generated output must stay gitignored; the lines above are residue this run created"
fi

# ------------------------------------------------------------------- timing --

printf '\n'
say "npm local-registry smoke: wall time ${SECONDS}s (budget ${WALL_BUDGET_SECONDS}s)"
if [ "$SECONDS" -gt "$WALL_BUDGET_SECONDS" ]; then
  warn "wall time ${SECONDS}s exceeds the ${WALL_BUDGET_SECONDS}s ledger ceiling for this line"
fi
