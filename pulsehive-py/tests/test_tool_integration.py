"""Integration tests for Python tool → agent → deploy pipeline."""

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


# ── Helper tools ──────────────────────────────────────────────────────

class CalculatorTool:
    """Simple calculator tool for testing."""
    def name(self): return "calculator"
    def description(self): return "Performs basic arithmetic"
    def parameters(self):
        return {
            "type": "object",
            "properties": {
                "operation": {"type": "string", "enum": ["add", "subtract"]},
                "a": {"type": "number"},
                "b": {"type": "number"},
            },
        }
    def execute(self, params, context):
        op = params.get("operation", "add")
        a = params.get("a", 0)
        b = params.get("b", 0)
        if op == "add":
            return str(a + b)
        elif op == "subtract":
            return str(a - b)
        return f"Unknown operation: {op}"


class FailingTool:
    """Tool that always raises an exception."""
    def name(self): return "exploder"
    def description(self): return "This tool always fails"
    def parameters(self): return {"type": "object"}
    def execute(self, params, context):
        raise RuntimeError("Intentional explosion for testing")


# ── Fixtures ──────────────────────────────────────────────────────────

@pytest.fixture
def hive(tmp_path):
    """Build a HiveMind with a fake OpenAI provider."""
    return (
        HiveMind.builder()
        .substrate_path(str(tmp_path / "test.db"))
        .llm_provider("openai", openai_provider("sk-test", "gpt-4"))
        .build()
    )


# ── Tests ─────────────────────────────────────────────────────────────

@pytest.mark.asyncio
async def test_deploy_agent_with_python_tool_does_not_crash(hive):
    """Agent with a Python tool should deploy without crashing."""
    lens = Lens(["test"])
    cfg = LlmConfig("openai", "gpt-4")
    kind = AgentKind.llm(
        "You are a calculator assistant.",
        lens, cfg,
        tools=[CalculatorTool()],
    )
    agent = AgentDefinition("calc-agent", kind)

    stream = await hive.deploy([agent], [Task("Calculate 2 + 3")])

    events = []
    async for event in stream:
        events.append(event)
        if event.event_type == "agent_completed":
            break

    types = [e.event_type for e in events]
    assert "agent_started" in types, f"Missing agent_started. Got: {types}"
    assert "agent_completed" in types, f"Missing agent_completed. Got: {types}"


@pytest.mark.asyncio
async def test_deploy_agent_with_failing_tool_completes_gracefully(hive):
    """Agent with a failing tool should still complete (error handled)."""
    lens = Lens(["test"])
    cfg = LlmConfig("openai", "gpt-4")
    kind = AgentKind.llm(
        "You test tools.",
        lens, cfg,
        tools=[FailingTool()],
    )
    agent = AgentDefinition("fail-agent", kind)

    stream = await hive.deploy([agent], [Task("Test the failing tool")])

    events = []
    async for event in stream:
        events.append(event)
        if event.event_type == "agent_completed":
            break

    # Agent should complete (not hang or crash)
    types = [e.event_type for e in events]
    assert "agent_completed" in types


@pytest.mark.asyncio
async def test_deploy_agent_with_multiple_tools(hive):
    """Agent with multiple Python tools should deploy cleanly."""
    lens = Lens(["test"])
    cfg = LlmConfig("openai", "gpt-4")
    kind = AgentKind.llm(
        "You have multiple tools.",
        lens, cfg,
        tools=[CalculatorTool(), FailingTool()],
    )
    agent = AgentDefinition("multi-tool-agent", kind)

    stream = await hive.deploy([agent], [Task("Use your tools")])

    events = []
    async for event in stream:
        events.append(event)
        if event.event_type == "agent_completed":
            break

    assert any(e.event_type == "agent_completed" for e in events)


@pytest.mark.asyncio
async def test_deploy_agent_no_tools_still_works(hive):
    """Agent without tools should still work (backward compat)."""
    lens = Lens(["test"])
    cfg = LlmConfig("openai", "gpt-4")
    kind = AgentKind.llm("You have no tools.", lens, cfg)
    agent = AgentDefinition("no-tools", kind)

    stream = await hive.deploy([agent], [Task("Say hello")])

    events = []
    async for event in stream:
        events.append(event)
        if event.event_type == "agent_completed":
            break

    assert any(e.event_type == "agent_completed" for e in events)


@pytest.mark.asyncio
async def test_tool_agent_event_data_has_correct_name(hive):
    """Agent started event should show the correct agent name."""
    lens = Lens(["test"])
    cfg = LlmConfig("openai", "gpt-4")
    kind = AgentKind.llm("Test.", lens, cfg, tools=[CalculatorTool()])
    agent = AgentDefinition("named-agent", kind)

    stream = await hive.deploy([agent], [Task("test")])

    async for event in stream:
        if event.event_type == "agent_started":
            assert event.data["name"] == "named-agent"
            break
