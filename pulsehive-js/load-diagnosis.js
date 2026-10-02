/**
 * load-diagnosis.js — why a native binding did not load, and what to say about it.
 *
 * `wrapper.js` requires this module and asks it one question: given this host
 * and the error the napi-generated loader threw, is the cause a host we never
 * advertised, an installation that is missing the binding, or a real load
 * failure that must be rethrown untouched (ADR-007, issue #101)?
 *
 * The advertised matrix lives here, moved out of `wrapper.js`, so the host
 * comparison and the message that names the matrix can never drift apart. It is
 * kept in step with `napi.targets` in package.json (ADR-015 A1).
 *
 * Host first. A host the matrix does not cover is `unsupported` whatever the
 * cause: a musl host loading the GNU binary fails with ERR_DLOPEN_FAILED, and
 * telling it to reinstall the GNU package would be wrong twice over.
 *
 * CommonJS and dependency-free: the wrapper requires it at load time, before
 * any binding exists.
 */
'use strict';

/**
 * Every host this package publishes a binding for. `libc` is only meaningful on
 * Linux; the non-Linux entries carry `null`, which matches any libc reported on
 * that platform.
 */
const ADVERTISED_TARGETS = [
  {
    triple: 'aarch64-apple-darwin',
    platform: 'darwin',
    arch: 'arm64',
    suffix: 'darwin-arm64',
    libc: null,
  },
  {
    triple: 'x86_64-unknown-linux-gnu',
    platform: 'linux',
    arch: 'x64',
    suffix: 'linux-x64-gnu',
    libc: 'glibc',
  },
  {
    triple: 'x86_64-pc-windows-msvc',
    platform: 'win32',
    arch: 'x64',
    suffix: 'win32-x64-msvc',
    libc: null,
  },
];

const MODULE_NOT_FOUND_CODES = ['MODULE_NOT_FOUND', 'ERR_MODULE_NOT_FOUND'];
const MODULE_NOT_FOUND_MESSAGE = /Cannot find module/;
// The generated loader reports both an unadvertised host and an advertised host
// whose platform package is not installed through this same outer message.
const LOADER_OUTER_MESSAGE = /Cannot find native binding|Failed to load native binding/;

/** True under vitest or `npm test`; the overrides below are test-only. */
function testEnvironment() {
  return process.env.NODE_ENV === 'test' || Boolean(process.env.VITEST);
}

function testOverride(name) {
  if (!testEnvironment()) return undefined;
  return process.env[name];
}

/**
 * 'glibc' when the runtime reports a glibc version, 'musl' on any other Linux,
 * and null off Linux (where libc does not pick a package). `PULSEHIVE_TEST_LIBC`
 * overrides it under test only, so a test can drive both Linux ABIs on one
 * machine.
 */
function detectLibc() {
  const override = testOverride('PULSEHIVE_TEST_LIBC');
  if (override !== undefined) return override === '' ? null : override;

  if (process.platform !== 'linux') return null;
  let glibcVersion;
  try {
    const report = typeof process.report?.getReport === 'function' ? process.report.getReport() : null;
    glibcVersion = report && report.header ? report.header.glibcVersionRuntime : undefined;
  } catch {
    glibcVersion = undefined;
  }
  return glibcVersion ? 'glibc' : 'musl';
}

/** The host as reported by the process; the three test overrides win under test. */
function detectHost() {
  return {
    platform: testOverride('PULSEHIVE_TEST_PLATFORM') || process.platform,
    arch: testOverride('PULSEHIVE_TEST_ARCH') || process.arch,
    libc: detectLibc(),
  };
}

/** The advertised entry for a host, or undefined when the matrix does not cover it. */
function advertisedTargetFor(platform, arch, libc) {
  return ADVERTISED_TARGETS.find(
    (target) =>
      target.platform === platform &&
      target.arch === arch &&
      (target.libc === null || target.libc === (libc ?? null)),
  );
}

/**
 * Every error reachable from `error`: the error itself, its `cause` (a single
 * error, an array of them, or a nested chain) and an AggregateError's `errors`.
 * The loader carries its per-attempt errors this way, and the shape varies.
 */
function collectErrors(error, seen = new Set(), collected = []) {
  if (error === null || error === undefined) return collected;
  if (Array.isArray(error)) {
    for (const item of error) collectErrors(item, seen, collected);
    return collected;
  }
  if (typeof error !== 'object' || seen.has(error)) return collected;
  seen.add(error);
  collected.push(error);

  collectErrors(error.cause, seen, collected);
  if (error instanceof AggregateError || error.name === 'AggregateError') {
    collectErrors(error.errors, seen, collected);
  }
  return collected;
}

function messageOf(error) {
  return typeof error.message === 'string' ? error.message : '';
}

/** True for the errors that mean "nothing to load", not "something failed". */
function isBenignLoadGap(error) {
  if (MODULE_NOT_FOUND_CODES.includes(error.code)) return true;
  const message = messageOf(error);
  return MODULE_NOT_FOUND_MESSAGE.test(message) || LOADER_OUTER_MESSAGE.test(message);
}

function hostLabel(host) {
  const libc = host.libc === 'musl' ? ' (musl)' : '';
  return `${host.platform}/${host.arch}${libc}`;
}

function advertisedTriples() {
  return ADVERTISED_TARGETS.map((target) => target.triple).join(', ');
}

/** ADR-015 A3's vocabulary: the host as platform/arch, and the advertised triples. */
function unsupportedHostMessage(host) {
  return (
    `@pulsehive/sdk does not support ${hostLabel(host)}: no native binding could be loaded ` +
    `for this host. Advertised targets: ${advertisedTriples()}. ` +
    `This host is not among them; build the native addon from source, or run ` +
    `on an advertised target.`
  );
}

function missingBindingMessage(target) {
  return (
    `@pulsehive/sdk cannot load the native binding for ${target.platform}/${target.arch}: ` +
    `no native binding could be loaded for this host. Advertised targets: ${advertisedTriples()}. ` +
    `This host is advertised as ${target.triple}, so the binding is missing from this ` +
    `installation: install @pulsehive/sdk-${target.suffix}, or reinstall @pulsehive/sdk ` +
    `so its optional dependencies resolve.`
  );
}

/**
 * Classify a load failure.
 *
 *   { kind: 'rethrow' }                        the original error is the truth
 *   { kind: 'unsupported', message }           no advertised target for this host
 *   { kind: 'missing', message }               advertised host, binding not installed
 *
 * The host is decided first, then the error chain: an unadvertised host stays
 * `unsupported` even when the cause is an ERR_DLOPEN_FAILED (a musl host loading
 * the GNU binary), and a chain that carries any real failure on an advertised
 * host is rethrown unchanged rather than rewritten as "reinstall".
 */
function diagnoseLoadFailure({ platform, arch, libc = null, error }) {
  const host = { platform, arch, libc: libc ?? null };
  const target = advertisedTargetFor(host.platform, host.arch, host.libc);
  if (!target) {
    return { kind: 'unsupported', message: unsupportedHostMessage(host) };
  }

  for (const collected of collectErrors(error)) {
    if (collected.code === 'ERR_DLOPEN_FAILED' || !isBenignLoadGap(collected)) {
      return { kind: 'rethrow' };
    }
  }
  return { kind: 'missing', message: missingBindingMessage(target) };
}

module.exports = {
  ADVERTISED_TARGETS,
  detectHost,
  detectLibc,
  advertisedTargetFor,
  diagnoseLoadFailure,
};
