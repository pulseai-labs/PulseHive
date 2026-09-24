/**
 * @pulsehive/sdk — TypeScript/Node.js bindings for PulseHive multi-agent SDK.
 *
 * This wrapper re-exports all napi-generated bindings and adds:
 * - Symbol.asyncIterator on EventStream for `for await` syntax
 * - defineTool() helper for ergonomic tool definition
 * - a named error when this host has no native binding (see below)
 */

// ── Advertised targets ───────────────────────────────────────────────
// The targets @pulsehive/sdk publishes binaries for. Kept in step with
// `napi.targets` in package.json: the generated index.js resolves each one as
// the package `@pulsehive/sdk-<suffix>`, pinned by exact version in
// optionalDependencies.
const ADVERTISED_TARGETS = [
  {
    triple: 'aarch64-apple-darwin',
    platform: 'darwin',
    arch: 'arm64',
    suffix: 'darwin-arm64',
  },
  {
    triple: 'x86_64-unknown-linux-gnu',
    platform: 'linux',
    arch: 'x64',
    suffix: 'linux-x64-gnu',
  },
  {
    triple: 'x86_64-pc-windows-msvc',
    platform: 'win32',
    arch: 'x64',
    suffix: 'win32-x64-msvc',
  },
];

function advertisedTargetFor(platform, arch) {
  return ADVERTISED_TARGETS.find(
    (target) => target.platform === platform && target.arch === arch,
  );
}

/**
 * True when `err` means "no native binding could be loaded here" rather than a
 * binding that was found and then failed. The generated loader reports both an
 * unadvertised host and a host whose platform package is not installed through
 * the same final `Cannot find native binding.` error, so that message — and a
 * bare module-resolution failure — is what identifies the class. A version
 * mismatch or a dlopen failure does not match, and is rethrown untouched.
 */
function isMissingNativeBinding(err) {
  if (!err) return false;
  if (err.code === 'MODULE_NOT_FOUND' || err.code === 'ERR_MODULE_NOT_FOUND') {
    return true;
  }
  const message = typeof err.message === 'string' ? err.message : '';
  return /Cannot find native binding|Failed to load native binding|Cannot find module/.test(
    message,
  );
}

function unsupportedHostError(cause) {
  const host = `${process.platform}/${process.arch}`;
  const triples = ADVERTISED_TARGETS.map((target) => target.triple).join(', ');
  const match = advertisedTargetFor(process.platform, process.arch);

  let message =
    `@pulsehive/sdk does not support ${host}: no native binding could be loaded ` +
    `for this host. Advertised targets: ${triples}.`;
  if (match) {
    message +=
      ` This host is advertised as ${match.triple}, so the binding is missing ` +
      `from this installation: install @pulsehive/sdk-${match.suffix}, or ` +
      `reinstall @pulsehive/sdk so its optional dependencies resolve.`;
  } else {
    message +=
      ` This host is not among them; build the native addon from source, or run ` +
      `on an advertised target.`;
  }

  const error = new Error(message);
  error.cause = cause;
  return error;
}

// Re-export everything from the napi-generated loader.
let napi;
try {
  napi = require('./index.js');
} catch (err) {
  if (!isMissingNativeBinding(err)) throw err;
  throw unsupportedHostError(err);
}

module.exports = { ...napi };

// ── Symbol.asyncIterator for EventStream ─────────────────────────────
// Enables: `for await (const event of stream) { ... }`
const EventStream = napi.EventStream;
if (EventStream && !EventStream.prototype[Symbol.asyncIterator]) {
  EventStream.prototype[Symbol.asyncIterator] = function () {
    const stream = this;
    return {
      async next() {
        const value = await stream.next();
        if (value === null || value === undefined) {
          return { done: true, value: undefined };
        }
        return { done: false, value };
      },
      [Symbol.asyncIterator]() {
        return this;
      },
    };
  };
}

// ── defineTool() — ergonomic tool definition ─────────────────────────
/**
 * Define a tool with a typed, ergonomic API.
 *
 * Instead of manually serializing JSON, use defineTool() to pass
 * a configuration object with parsed params and context:
 *
 * ```typescript
 * const calculator = defineTool({
 *   name: 'calculator',
 *   description: 'Performs arithmetic',
 *   parameters: {
 *     type: 'object',
 *     properties: { expression: { type: 'string' } },
 *     required: ['expression'],
 *   },
 *   execute: async (params, context) => {
 *     return `Result: ${params.expression}`;
 *   },
 * });
 * ```
 */
function defineTool(config) {
  const { name, description, parameters, execute, requiresApproval } = config;
  const parametersJson = JSON.stringify(parameters);

  // Wrap the user's typed callback to handle JSON serialization
  const wrappedExecute = async (payloadJson) => {
    const payload = JSON.parse(payloadJson);
    const result = await execute(payload.params, payload.context);
    // If result is an object, stringify it for the Rust side
    if (typeof result === 'object' && result !== null) {
      return JSON.stringify(result);
    }
    return String(result);
  };

  return new napi.Tool(
    name,
    description,
    parametersJson,
    wrappedExecute,
    requiresApproval ?? false,
  );
}

module.exports.defineTool = defineTool;
