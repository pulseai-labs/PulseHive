"""PulseHive Multi-Agent Workflows — Sequential and Parallel agents.

This example demonstrates:
1. Sequential workflow: agents run in order, each perceiving previous results
2. Parallel workflow: agents run concurrently, sharing the substrate
3. Nested workflows: combining Sequential and Parallel

Prerequisites:
    pip install pulsehive
    export OPENAI_API_KEY="sk-..."

Usage:
    python examples/multi_agent.py
"""

import asyncio
import os

from pulsehive import (
    AgentDefinition,
    AgentKind,
    HiveMind,
    Lens,
    LlmConfig,
    Task,
    openai_provider,
)


async def main():
    api_key = os.environ.get("OPENAI_API_KEY", "")

    hive = (
        HiveMind.builder()
        .substrate_path("/tmp/pulsehive_multi_agent.db")
        .llm_provider("openai", openai_provider(api_key, "gpt-4"))
        .build()
    )

    config = LlmConfig("openai", "gpt-4")

    # ── Sequential Workflow ───────────────────────────────────────────
    # Step 1 analyzes, Step 2 summarizes (perceiving Step 1's results)
    print("=== Sequential Pipeline ===")

    researcher = AgentDefinition(
        "researcher",
        AgentKind.llm(
            "You research topics thoroughly. Provide detailed findings.",
            Lens(["research"]),
            config,
        ),
    )

    summarizer = AgentDefinition(
        "summarizer",
        AgentKind.llm(
            "You summarize research findings into concise bullet points.",
            Lens(["research", "summary"]),
            config,
        ),
    )

    pipeline = AgentDefinition(
        "research-pipeline",
        AgentKind.sequential([researcher, summarizer]),
    )

    stream = await hive.deploy([pipeline], [Task("Research Python async patterns")])
    await _consume_events(stream)

    # ── Parallel Workflow ─────────────────────────────────────────────
    # Two agents work concurrently on different aspects
    print("\n=== Parallel Team ===")

    frontend_reviewer = AgentDefinition(
        "frontend-reviewer",
        AgentKind.llm(
            "You review frontend code for best practices.",
            Lens(["frontend", "ui"]),
            config,
        ),
    )

    backend_reviewer = AgentDefinition(
        "backend-reviewer",
        AgentKind.llm(
            "You review backend code for performance and security.",
            Lens(["backend", "security"]),
            config,
        ),
    )

    review_team = AgentDefinition(
        "review-team",
        AgentKind.parallel([frontend_reviewer, backend_reviewer]),
    )

    stream = await hive.deploy([review_team], [Task("Review the web application")])
    await _consume_events(stream)

    # ── Nested Workflow ───────────────────────────────────────────────
    # Parallel analysis → Sequential summary
    print("\n=== Nested: Parallel Analysis → Summary ===")

    analyst_a = AgentDefinition("analyst-a", AgentKind.llm("Analyze performance.", Lens(["perf"]), config))
    analyst_b = AgentDefinition("analyst-b", AgentKind.llm("Analyze security.", Lens(["security"]), config))

    combined = AgentDefinition(
        "full-review",
        AgentKind.sequential([
            AgentDefinition("parallel-analysis", AgentKind.parallel([analyst_a, analyst_b])),
            AgentDefinition("final-summary", AgentKind.llm("Summarize all findings.", Lens([]), config)),
        ]),
    )

    stream = await hive.deploy([combined], [Task("Full system review")])
    await _consume_events(stream)

    hive.shutdown()
    print("\nDone!")


async def _consume_events(stream):
    """Consume and display events from the stream."""
    async for event in stream:
        agent_id = event.agent_id or ""
        short_id = agent_id[:8] if agent_id else ""
        print(f"  [{event.event_type}] {short_id} {_summary(event)}")
        if event.event_type == "agent_completed" and "pipeline" not in str(event.data.get("name", "")):
            # Stop after top-level agent completes
            data = event.data
            if data.get("outcome") == "complete":
                break
    # Drain remaining events
    async for event in stream:
        if event.event_type == "agent_completed":
            break


def _summary(event):
    data = event.data
    if event.event_type == "agent_started":
        return f"→ {data.get('name')} ({data.get('kind')})"
    elif event.event_type == "agent_completed":
        return f"✓ {data.get('outcome')}"
    return ""


if __name__ == "__main__":
    asyncio.run(main())
