"""AC-4 (r2.s5.w1) — the Python binding surfaces a degraded Sequential.

`Sequential([Parallel([ok, failing]), critic])` where the failing lens's
provider answers a non-retryable HTTP 400: the critic still runs (a partial
child is progress) and the sequence's ``AgentCompleted`` maps to
``outcome == "partial_complete"`` carrying the critic's text in ``responses``
and exactly one error naming the failing agent — never ``"complete"``
(a downstream consumer's smoke finding PH-1; ADR-014's r2.s5 amendment, L1/L2).

Offline by construction: both OpenAI-compatible providers point at a stub HTTP
server this test starts on ``127.0.0.1`` with an OS-assigned port. No real
network, no API key. Two providers with different base paths route the traffic
— ``/bad/...`` is the failing lens (a terminal 400, so the provider does not
retry), everything else answers a valid chat completion — and the stub picks
the text by the request's model name so the critic's own response is
distinguishable from the survivor's.
"""

import asyncio
import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import pytest

from pulsehive import (
    AgentDefinition,
    AgentKind,
    HiveMind,
    Lens,
    LlmConfig,
    Task,
    openai_provider,
)

SURVIVOR_TEXT = "survivor text"
CRITIC_TEXT = "critique done"

# Every (path, model) pair the stub was asked for — the evidence that the
# failing lens really reached the 400 and that the critic really ran.
REQUESTS: list = []


class _StubHandler(BaseHTTPRequestHandler):
    """A chat-completions stub: `/bad/...` is the failing lens, `/ok/...` answers."""

    protocol_version = "HTTP/1.1"

    def do_POST(self):  # noqa: N802 — http.server's method naming
        length = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(length)
        try:
            model = json.loads(raw or b"{}").get("model")
        except json.JSONDecodeError:
            model = None
        REQUESTS.append((self.path, model))

        if self.path.startswith("/bad"):
            # Non-retryable: a 400 is a terminal client error for the provider,
            # so the failing lens fails once — no retry storm.
            body = json.dumps(
                {"error": {"message": "lens unavailable", "type": "invalid_request_error"}}
            ).encode()
            self.send_response(400)
        else:
            text = CRITIC_TEXT if model == "critic-model" else SURVIVOR_TEXT
            body = json.dumps(
                {
                    "id": "chatcmpl-stub",
                    "object": "chat.completion",
                    "created": 0,
                    "model": model or "stub-model",
                    "choices": [
                        {
                            "index": 0,
                            "message": {"role": "assistant", "content": text},
                            "finish_reason": "stop",
                        }
                    ],
                    "usage": {"prompt_tokens": 1, "completion_tokens": 1},
                }
            ).encode()
            self.send_response(200)

        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):  # keep the test output clean
        pass


def _start_stub():
    """Start the stub on 127.0.0.1 with an OS-assigned port; return (server, port)."""
    server = ThreadingHTTPServer(("127.0.0.1", 0), _StubHandler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    return server, server.server_address[1]


def _llm_child(name, provider, model):
    """An LLM agent definition routing to the named provider."""
    return AgentDefinition(
        name,
        AgentKind.llm("Work the task.", Lens(["test"]), LlmConfig(provider, model)),
    )


def _outcome_data(events, agent_name):
    """The ``AgentCompleted`` data dict of the named agent (by its agent_id)."""
    target = None
    for event in events:
        if event.event_type == "agent_started" and event.data.get("name") == agent_name:
            target = event.agent_id
    assert target is not None, f"{agent_name} never started; saw {[e.event_type for e in events]}"
    for event in events:
        if event.event_type == "agent_completed" and event.agent_id == target:
            return event.data
    raise AssertionError(f"{agent_name} never completed; saw {[e.event_type for e in events]}")


async def _drain_until_completed(stream, agent_name, timeout=30.0):
    """Drain the deploy stream until the named agent completes — bounded, so a
    hung run fails the test instead of hanging the suite."""

    async def pump():
        events = []
        target = None
        async for event in stream:
            if (
                event.event_type == "agent_started"
                and event.data.get("name") == agent_name
            ):
                target = event.agent_id
            events.append(event)
            if (
                event.event_type == "agent_completed"
                and target is not None
                and event.agent_id == target
            ):
                break
        return events

    return await asyncio.wait_for(pump(), timeout)


@pytest.mark.asyncio
async def test_degraded_sequential_maps_to_partial_complete(tmp_path):
    """One failing lens degrades the whole Sequential — surfaced by the binding."""
    server, port = _start_stub()
    REQUESTS.clear()
    try:
        hive = (
            HiveMind.builder()
            .substrate_path(str(tmp_path / "degraded-sequential.db"))
            .llm_provider(
                "ok",
                openai_provider(
                    "sk-test", "survivor-model", f"http://127.0.0.1:{port}/ok"
                ),
            )
            .llm_provider(
                "bad",
                openai_provider(
                    "sk-test", "failing-model", f"http://127.0.0.1:{port}/bad"
                ),
            )
            .build()
        )
        try:
            workflow = AgentDefinition(
                "seq-degraded",
                AgentKind.sequential(
                    [
                        AgentDefinition(
                            "par-stage",
                            AgentKind.parallel(
                                [
                                    _llm_child("survivor", "ok", "survivor-model"),
                                    _llm_child("failing", "bad", "failing-model"),
                                ]
                            ),
                        ),
                        _llm_child("critic", "ok", "critic-model"),
                    ]
                ),
            )
            stream = await hive.deploy([workflow], [Task("degraded sequential")])
            events = await _drain_until_completed(stream, "seq-degraded")
        finally:
            hive.shutdown()
    finally:
        server.shutdown()
        server.server_close()

    sequence = _outcome_data(events, "seq-degraded")
    assert sequence["outcome"] == "partial_complete", (
        f"a degraded Sequential must not report a clean run: {sequence}"
    )
    assert json.loads(sequence["responses"]) == [CRITIC_TEXT], (
        "the degraded sequence's responses are what its Complete would have "
        f"carried: {sequence['responses']}"
    )
    errors = json.loads(sequence["errors"])
    assert len(errors) == 1, f"one error for the one failed lens: {errors}"
    assert errors[0].startswith("failing: "), (
        f"the error must name the failed agent, got {errors[0]!r}"
    )

    # The failing child's own outcome is still attributable, and the critic is
    # what the sequence's responses hold — the sequence continued past it.
    stage = _outcome_data(events, "par-stage")
    assert stage["outcome"] == "partial_complete"
    assert json.loads(stage["responses"]) == [SURVIVOR_TEXT]
    assert json.loads(stage["errors"]) == errors

    # Tamper evidence: the stub really carried the traffic.
    seen = list(REQUESTS)
    assert any(path.startswith("/bad/") for path, _ in seen), (
        f"the failing lens never reached the stub: {seen}"
    )
    assert any(model == "critic-model" for _, model in seen), (
        f"the critic never ran — the sequence stopped at the partial child: {seen}"
    )
