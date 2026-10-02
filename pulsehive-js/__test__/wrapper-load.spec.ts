/**
 * wrapper-load.spec.ts — the wrapper's load path, end to end, with no binding.
 *
 * `wrapper.js` is copied into a temp dir beside a stub `index.js` that throws a
 * chosen error, and the copy is `require`d. The real wrapper code runs — its
 * `require('./index.js')` fails as it would on a broken install, and whatever it
 * throws (or rethrows) comes straight back to the assertions here, where the
 * original objects are still in hand. No native build, no registry.
 */
import { copyFileSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { afterEach, describe, expect, it } from 'vitest';

const require = createRequire(import.meta.url);
const PACKAGE_DIR = resolve(dirname(fileURLToPath(import.meta.url)), '..');

/** Throws whatever object the test put on the global — the loader's own throw. */
const STUB_INDEX = `'use strict';
const stub = globalThis.__PULSEHIVE_STUB_INDEX__;
if (!stub || !stub.error) {
  throw new Error('wrapper-load.spec: the stub index.js was required without an error');
}
throw stub.error;
`;

const STUB_KEY = '__PULSEHIVE_STUB_INDEX__';
const ENV_KEYS = [
  'NODE_ENV',
  'VITEST',
  'PULSEHIVE_TEST_PLATFORM',
  'PULSEHIVE_TEST_ARCH',
  'PULSEHIVE_TEST_LIBC',
] as const;

type CodedError = Error & { code?: string; cause?: unknown };
type Stub = { error: unknown };
type GlobalWithStub = typeof globalThis & { [STUB_KEY]?: Stub };

const temporaryDirs: string[] = [];

afterEach(() => {
  for (const dir of temporaryDirs.splice(0)) {
    rmSync(dir, { recursive: true, force: true });
  }
  delete (globalThis as GlobalWithStub)[STUB_KEY];
});

/** The generated loader's outer error, wrapping its per-attempt causes. */
function loaderError(cause?: unknown): CodedError {
  const error = new Error(
    'Cannot find native binding. npm has a bug related to optional dependencies ' +
      '(https://github.com/npm/cli/issues/4828). Please try `npm i` again after ' +
      'removing both package-lock.json and node_modules directory.',
  ) as CodedError;
  if (cause !== undefined) error.cause = cause;
  return error;
}

function moduleNotFound(): CodedError {
  const error = new Error("Cannot find module '@pulsehive/sdk-linux-x64-gnu'") as CodedError;
  error.code = 'MODULE_NOT_FOUND';
  return error;
}

function dlopenFailed(): CodedError {
  const error = new Error(
    '/app/node_modules/@pulsehive/sdk-linux-x64-gnu/pulsehive-js.linux-x64-gnu.node: ' +
      'cannot open shared object file: No such file or directory',
  ) as CodedError;
  error.code = 'ERR_DLOPEN_FAILED';
  return error;
}

const NEVER_THREW = Symbol('never-threw');

/**
 * Requires a copy of the real `wrapper.js` with the stub in place, and returns
 * what it threw. Pins the host through the test overrides so the result does not
 * depend on the machine running the suite.
 */
function requireWrapperCopy(error: unknown, libc: 'glibc' | 'musl'): unknown {
  const dir = mkdtempSync(join(tmpdir(), 'pulsehive-wrapper-load-'));
  temporaryDirs.push(dir);
  copyFileSync(join(PACKAGE_DIR, 'wrapper.js'), join(dir, 'wrapper.js'));
  copyFileSync(join(PACKAGE_DIR, 'load-diagnosis.js'), join(dir, 'load-diagnosis.js'));
  writeFileSync(join(dir, 'index.js'), STUB_INDEX);

  const previous = new Map<string, string | undefined>();
  for (const key of ENV_KEYS) previous.set(key, process.env[key]);
  process.env.NODE_ENV = 'test';
  process.env.PULSEHIVE_TEST_PLATFORM = 'linux';
  process.env.PULSEHIVE_TEST_ARCH = 'x64';
  process.env.PULSEHIVE_TEST_LIBC = libc;
  (globalThis as GlobalWithStub)[STUB_KEY] = { error };

  try {
    require(join(dir, 'wrapper.js'));
    return NEVER_THREW;
  } catch (thrown) {
    return thrown;
  } finally {
    for (const key of ENV_KEYS) {
      const value = previous.get(key);
      if (value === undefined) delete process.env[key];
      else process.env[key] = value;
    }
  }
}

describe('wrapper.js load path', () => {
  it('rethrows the original error object when the binding itself failed to load', () => {
    // A real defect (ERR_DLOPEN_FAILED) under the loader's outer message: the
    // old outer-message match rewrote this as "reinstall" (ADR-007, #101).
    const outer = loaderError([moduleNotFound(), dlopenFailed()]);
    const thrown = requireWrapperCopy(outer, 'glibc');
    expect(thrown).toBe(outer);
  });

  it('throws a new error with the original as its cause when the platform package is missing', () => {
    const outer = loaderError([moduleNotFound()]);
    const thrown = requireWrapperCopy(outer, 'glibc') as CodedError;
    expect(thrown).not.toBe(outer);
    expect(thrown).toBeInstanceOf(Error);
    expect(thrown.cause).toBe(outer);
    expect(thrown.message).toContain('@pulsehive/sdk-linux-x64-gnu');
  });

  it('names a musl host as unsupported instead of advising the GNU package', () => {
    const outer = loaderError([dlopenFailed()]);
    const thrown = requireWrapperCopy(outer, 'musl') as CodedError;
    expect(thrown).not.toBe(outer);
    expect(thrown.cause).toBe(outer);
    expect(thrown.message).toContain('musl');
    expect(thrown.message).not.toContain('sdk-linux-x64-gnu');
  });
});
