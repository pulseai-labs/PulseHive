/**
 * @pulsehive/sdk — TypeScript/Node.js bindings for PulseHive multi-agent SDK.
 *
 * This wrapper re-exports all napi-generated bindings and adds:
 * - Symbol.asyncIterator on EventStream for `for await` syntax
 * - defineTool() helper for ergonomic tool definition
 * - why a failed `require` failed: a named error when this installation is
 *   missing the binding, the original error when the binding itself did not
 *   load (see load-diagnosis.js, #101)
 */

// ── Why a binding did not load ───────────────────────────────────────
// The advertised matrix and the classification of a failed `require` live in
// load-diagnosis.js: an unadvertised host (musl included) is told so, an
// advertised host whose platform package is missing is told which one to
// install, and anything else — a binding that was found and then failed to load
// — is rethrown exactly as it arrived (ADR-007, #101).
const { detectHost, diagnoseLoadFailure } = require('./load-diagnosis.js');

// Re-export everything from the napi-generated loader.
let napi;
try {
  napi = require('./index.js');
} catch (err) {
  const diagnosis = diagnoseLoadFailure({ ...detectHost(), error: err });
  if (diagnosis.kind === 'rethrow') throw err;
  throw new Error(diagnosis.message, { cause: err });
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
