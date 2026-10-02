/**
 * load-diagnosis.spec.ts — why a native binding did not load (issue #101).
 *
 * Pure-function coverage: every case hands `diagnoseLoadFailure` an explicit
 * `platform` / `arch` / `libc`, so nothing here depends on the machine running
 * the tests and no native build is needed — the module under test is plain
 * CommonJS.
 */
import { describe, expect, it } from 'vitest';

import { ADVERTISED_TARGETS, diagnoseLoadFailure } from '../load-diagnosis.js';

type CodedError = Error & { code?: string; cause?: unknown };

/** The generated loader's outer error: every attempt is wrapped by this. */
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

/** A binding that was found and then failed to load — a real defect, not a gap. */
function dlopenFailed(): CodedError {
  const error = new Error(
    '/app/node_modules/@pulsehive/sdk-linux-x64-gnu/pulsehive-js.linux-x64-gnu.node: ' +
      'cannot open shared object file: No such file or directory',
  ) as CodedError;
  error.code = 'ERR_DLOPEN_FAILED';
  return error;
}

const glibcHost = { platform: 'linux', arch: 'x64', libc: 'glibc' } as const;

describe('diagnoseLoadFailure', () => {
  it('reports a missing binding on an advertised glibc host (module-not-found cause)', () => {
    const result = diagnoseLoadFailure({
      ...glibcHost,
      error: loaderError(moduleNotFound()),
    });
    expect(result.kind).toBe('missing');
    expect(result.message).toContain('@pulsehive/sdk-linux-x64-gnu');
  });

  it('reports an unadvertised musl host as unsupported, and never advises the GNU package', () => {
    const result = diagnoseLoadFailure({
      platform: 'linux',
      arch: 'x64',
      libc: 'musl',
      error: loaderError(moduleNotFound()),
    });
    expect(result.kind).toBe('unsupported');
    expect(result.message).toContain('musl');
    expect(result.message).toContain('linux/x64');
    expect(result.message).not.toContain('sdk-linux-x64-gnu');
  });

  it('reports an unadvertised linux/arm64 host as unsupported', () => {
    const result = diagnoseLoadFailure({
      platform: 'linux',
      arch: 'arm64',
      libc: 'glibc',
      error: loaderError(moduleNotFound()),
    });
    expect(result.kind).toBe('unsupported');
    expect(result.message).toContain('linux/arm64');
  });

  it('rethrows a real load failure hidden behind the loader outer message', () => {
    const result = diagnoseLoadFailure({
      ...glibcHost,
      error: loaderError(dlopenFailed()),
    });
    expect(result.kind).toBe('rethrow');
  });

  it('rethrows an unrelated error, wrapped or bare', () => {
    const wrapped = diagnoseLoadFailure({
      ...glibcHost,
      error: loaderError(new TypeError('sdk.version is not a function')),
    });
    expect(wrapped.kind).toBe('rethrow');

    const bare = diagnoseLoadFailure({
      ...glibcHost,
      error: new TypeError('sdk.version is not a function'),
    });
    expect(bare.kind).toBe('rethrow');
  });

  it('reports a missing binding on darwin/arm64', () => {
    const result = diagnoseLoadFailure({
      platform: 'darwin',
      arch: 'arm64',
      libc: null,
      error: loaderError(moduleNotFound()),
    });
    expect(result.kind).toBe('missing');
    expect(result.message).toContain('@pulsehive/sdk-darwin-arm64');
  });

  it('checks the host before the error: musl with a dlopen failure is still unsupported', () => {
    const result = diagnoseLoadFailure({
      platform: 'linux',
      arch: 'x64',
      libc: 'musl',
      error: loaderError(dlopenFailed()),
    });
    expect(result.kind).toBe('unsupported');
    expect(result.message).toContain('musl');
    expect(result.message).not.toContain('sdk-linux-x64-gnu');
  });

  it('walks AggregateError.errors for a dlopen failure', () => {
    const aggregate = new AggregateError(
      [moduleNotFound(), dlopenFailed()],
      'native binding load attempts failed',
    );
    expect(
      diagnoseLoadFailure({ ...glibcHost, error: loaderError(aggregate) }).kind,
    ).toBe('rethrow');
    expect(diagnoseLoadFailure({ ...glibcHost, error: aggregate }).kind).toBe('rethrow');
  });

  it('walks nested module-not-found causes to a missing binding', () => {
    const nested = loaderError(loaderError([moduleNotFound(), moduleNotFound()]));
    const result = diagnoseLoadFailure({ ...glibcHost, error: nested });
    expect(result.kind).toBe('missing');
    expect(result.message).toContain('@pulsehive/sdk-linux-x64-gnu');
  });

  it('carries the advertised matrix with its libc field', () => {
    const libcBySuffix: Record<string, string | null> = {};
    for (const target of ADVERTISED_TARGETS as Array<{ suffix: string; libc: string | null }>) {
      libcBySuffix[target.suffix] = target.libc;
    }
    expect(libcBySuffix['linux-x64-gnu']).toBe('glibc');
    expect(libcBySuffix['darwin-arm64']).toBeNull();
    expect(libcBySuffix['win32-x64-msvc']).toBeNull();
  });
});
