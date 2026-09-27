/// AC-5 (r2.s5.w1) — the JavaScript binding surfaces a degraded Sequential.
///
/// `Sequential([Parallel([ok, failing]), critic])` where the failing lens's
/// provider answers a non-retryable HTTP 400: the critic still runs (a partial
/// child is progress) and the sequence's `AgentCompleted` maps to
/// `outcome === "partial_complete"` carrying the critic's text in `responses`
/// and exactly one error naming the failing agent — never `"complete"`
/// (pulse-guard PH-1; ADR-014's r2.s5 amendment, L1/L2).
///
/// Offline by construction: both OpenAI-compatible providers point at a stub
/// `node:http` server this test starts on `127.0.0.1` with an OS-assigned port.
/// No real network, no API key. Two providers with different base paths route
/// the traffic — `/bad/...` is the failing lens (a terminal 400, so the provider
/// does not retry), everything else answers a valid chat completion — and the
/// stub picks the text by the request's model name so the critic's own response
/// is distinguishable from the survivor's.

import { describe, it, expect, afterEach } from "vitest";

const http = require("node:http");
const {
  HiveMind,
  Task,
  JsAgentKind: AgentKind,
  JsAgentDefinition: AgentDefinition,
  JsLens: Lens,
  JsLlmConfig: LlmConfig,
  openaiProvider,
} = require("../wrapper.js");

const SURVIVOR_TEXT = "survivor text";
const CRITIC_TEXT = "critique done";

/// Every (path, model) pair the stub was asked for — the evidence that the
/// failing lens really reached the 400 and that the critic really ran.
let requests: Array<{ path: string; model: string | null }> = [];
let servers: any[] = [];
let hives: any[] = [];

/// Start the chat-completions stub on 127.0.0.1 with an OS-assigned port.
function startStub(): Promise<{ server: any; port: number }> {
  const server = http.createServer((req: any, res: any) => {
    let raw = "";
    req.on("data", (chunk: any) => (raw += chunk));
    req.on("end", () => {
      let model: string | null = null;
      try {
        model = JSON.parse(raw || "{}").model ?? null;
      } catch {
        model = null;
      }
      const path: string = req.url ?? "";
      requests.push({ path, model });

      let status = 200;
      let body: string;
      if (path.startsWith("/bad")) {
        // Non-retryable: a 400 is a terminal client error for the provider, so
        // the failing lens fails once — no retry storm.
        status = 400;
        body = JSON.stringify({
          error: { message: "lens unavailable", type: "invalid_request_error" },
        });
      } else {
        body = JSON.stringify({
          id: "chatcmpl-stub",
          object: "chat.completion",
          created: 0,
          model: model ?? "stub-model",
          choices: [
            {
              index: 0,
              message: {
                role: "assistant",
                content: model === "critic-model" ? CRITIC_TEXT : SURVIVOR_TEXT,
              },
              finish_reason: "stop",
            },
          ],
          usage: { prompt_tokens: 1, completion_tokens: 1 },
        });
      }
      res.writeHead(status, {
        "Content-Type": "application/json",
        "Content-Length": Buffer.byteLength(body),
      });
      res.end(body);
    });
  });
  return new Promise((resolve) => {
    server.listen(0, "127.0.0.1", () => {
      resolve({ server, port: server.address().port });
    });
  });
}

/// An LLM agent definition routing to the named provider.
function llmChild(name: string, provider: string, model: string) {
  return new AgentDefinition(
    name,
    AgentKind.llm("Work the task.", new Lens(["test"]), new LlmConfig(provider, model)),
  );
}

/// The `agent_completed` data of the named agent (found by its `agentId`).
function outcomeData(events: any[], agentName: string): Record<string, string> {
  let target: string | null = null;
  for (const event of events) {
    if (event.eventType === "agent_started" && event.data.name === agentName) {
      target = event.agentId;
    }
  }
  expect(target, `${agentName} never started`).not.toBeNull();
  for (const event of events) {
    if (event.eventType === "agent_completed" && event.agentId === target) {
      return event.data;
    }
  }
  throw new Error(`${agentName} never completed`);
}

afterEach(() => {
  for (const hive of hives) {
    try {
      if (hive && !hive.isShutdown) hive.shutdown();
    } catch {
      // shutdown is best-effort in cleanup
    }
  }
  hives = [];
  for (const server of servers) {
    try {
      server.close();
    } catch {
      // close is best-effort in cleanup
    }
  }
  servers = [];
});

describe("degraded Sequential (PH-1)", () => {
  it("maps one failed lens to partial_complete with the critic's text and a named error", async () => {
    requests = [];
    const { server, port } = await startStub();
    servers.push(server);

    const hive = HiveMind.builder()
      .substratePath(`/tmp/pulsehive-degraded-seq-${Date.now()}.db`)
      .llmProvider(
        "ok",
        openaiProvider("sk-test", "survivor-model", `http://127.0.0.1:${port}/ok`),
      )
      .llmProvider(
        "bad",
        openaiProvider("sk-test", "failing-model", `http://127.0.0.1:${port}/bad`),
      )
      .build();
    hives.push(hive);

    const workflow = new AgentDefinition(
      "seq-degraded",
      AgentKind.sequential([
        new AgentDefinition(
          "par-stage",
          AgentKind.parallel([
            llmChild("survivor", "ok", "survivor-model"),
            llmChild("failing", "bad", "failing-model"),
          ]),
        ),
        llmChild("critic", "ok", "critic-model"),
      ]),
    );

    const stream = await hive.deploy([workflow], [new Task("degraded sequential")]);

    // Drain until the sequence's own AgentCompleted.
    const events: any[] = [];
    let target: string | null = null;
    for (;;) {
      const event = await stream.next();
      if (event === null) break;
      if (event.eventType === "agent_started" && event.data.name === "seq-degraded") {
        target = event.agentId;
      }
      events.push(event);
      if (
        event.eventType === "agent_completed" &&
        target !== null &&
        event.agentId === target
      ) {
        break;
      }
    }

    const sequence = outcomeData(events, "seq-degraded");
    expect(sequence.outcome).toBe("partial_complete");
    expect(JSON.parse(sequence.responses)).toEqual([CRITIC_TEXT]);
    const errors = JSON.parse(sequence.errors);
    expect(errors).toHaveLength(1);
    expect(errors[0]).toMatch(/^failing: /);

    // The failing child's own outcome is still attributable, and the critic is
    // what the sequence's responses hold — the sequence continued past it.
    const stage = outcomeData(events, "par-stage");
    expect(stage.outcome).toBe("partial_complete");
    expect(JSON.parse(stage.responses)).toEqual([SURVIVOR_TEXT]);
    expect(JSON.parse(stage.errors)).toEqual(errors);

    // Tamper evidence: the stub really carried the traffic.
    expect(requests.some((r) => r.path.startsWith("/bad/"))).toBe(true);
    expect(requests.some((r) => r.model === "critic-model")).toBe(true);
  });
});
