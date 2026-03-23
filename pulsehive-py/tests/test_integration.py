"""Integration tests for PulseHive async deploy pipeline."""

import asyncio

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


@pytest.fixture
def hive(tmp_path):
    """Build a HiveMind with a fake OpenAI provider for testing."""
    return (
        HiveMind.builder()
        .substrate_path(str(tmp_path / "test.db"))
        .llm_provider("openai", openai_provider("sk-test", "gpt-4"))
        .build()
    )


@pytest.fixture
def agent():
    """Create a simple LLM agent."""
    lens = Lens(["test"])
    cfg = LlmConfig("openai", "gpt-4")
    kind = AgentKind.llm("You are a test agent.", lens, cfg)
    return AgentDefinition("test-agent", kind)


@pytest.mark.asyncio
async def test_deploy_returns_event_stream(hive, agent):
    """Deploy an agent and verify we get an EventStream back."""
    stream = await hive.deploy([agent], [Task("Say hello")])
    assert repr(stream) == "EventStream(active)"


@pytest.mark.asyncio
async def test_deploy_emits_lifecycle_events(hive, agent):
    """Deploy an agent and verify lifecycle events are emitted."""
    stream = await hive.deploy([agent], [Task("Test task")])

    events = []
    async for event in stream:
        events.append(event)
        if event.event_type == "agent_completed":
            break

    types = [e.event_type for e in events]
    assert "agent_started" in types, f"Missing agent_started. Got: {types}"
    assert "agent_completed" in types, f"Missing agent_completed. Got: {types}"


@pytest.mark.asyncio
async def test_deploy_event_data_accessible(hive, agent):
    """Verify event data dict is accessible and has expected keys."""
    stream = await hive.deploy([agent], [Task("Test")])

    async for event in stream:
        if event.event_type == "agent_started":
            data = event.data
            assert "agent_id" in data
            assert "name" in data
            assert data["name"] == "test-agent"
            break


@pytest.mark.asyncio
async def test_deploy_agent_id_property(hive, agent):
    """Verify agent_id convenience property works."""
    stream = await hive.deploy([agent], [Task("Test")])

    async for event in stream:
        if event.event_type == "agent_started":
            assert event.agent_id is not None
            assert isinstance(event.agent_id, str)
            break


@pytest.mark.asyncio
async def test_deploy_empty_agents(hive):
    """Empty agent list should return an empty stream."""
    stream = await hive.deploy([], [Task("Nothing")])

    events = []
    async for event in stream:
        events.append(event)

    assert len(events) == 0


@pytest.mark.asyncio
async def test_deploy_agent_completes_with_error(hive, agent):
    """Agent should complete with error since the LLM API key is fake."""
    stream = await hive.deploy([agent], [Task("Test")])

    async for event in stream:
        if event.event_type == "agent_completed":
            data = event.data
            # Agent should have completed (possibly with error due to fake API key)
            assert data["outcome"] in ("complete", "error")
            break
