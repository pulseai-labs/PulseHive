"""PulseHive Getting Started — Deploy a single LLM agent.

This example demonstrates:
1. Building a HiveMind with substrate and LLM provider
2. Defining an agent with a Lens and LlmConfig
3. Deploying the agent and consuming the async event stream

Prerequisites:
    pip install pulsehive
    export OPENAI_API_KEY="sk-..."   # or use anthropic_provider

Usage:
    python examples/getting_started.py
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
    # 1. Get API key from environment
    api_key = os.environ.get("OPENAI_API_KEY", "")
    if not api_key:
        print("Set OPENAI_API_KEY environment variable to run this example.")
        print("Running with empty key (agent will error, but pipeline works).")

    # 2. Build HiveMind — the orchestrator
    hive = (
        HiveMind.builder()
        .substrate_path("/tmp/pulsehive_getting_started.db")
        .llm_provider("openai", openai_provider(api_key, "gpt-4"))
        .build()
    )

    # 3. Define perception lens — what the agent pays attention to
    lens = Lens(
        ["code", "architecture"],  # Domain focus
        attention_budget=50,       # Max experiences to perceive per cycle
    )

    # 4. Configure LLM selection
    config = LlmConfig("openai", "gpt-4", temperature=0.7, max_tokens=2048)

    # 5. Create an LLM agent
    kind = AgentKind.llm(
        system_prompt="You are a helpful code analysis assistant. Analyze code structure and suggest improvements.",
        lens=lens,
        llm_config=config,
    )
    agent = AgentDefinition("code-analyzer", kind)

    # 6. Deploy and consume events
    print(f"Deploying agent '{agent.name}'...")
    stream = await hive.deploy([agent], [Task("Analyze the project structure")])

    async for event in stream:
        print(f"  [{event.event_type}] {_format_event(event)}")
        if event.event_type == "agent_completed":
            data = event.data
            if data.get("outcome") == "complete":
                print(f"\nAgent response:\n{data.get('response', '')[:500]}")
            else:
                print(f"\nAgent finished with: {data.get('outcome', 'unknown')}")
            break

    # 7. Cleanup
    hive.shutdown()
    print("\nDone!")


def _format_event(event):
    """Format event for display."""
    data = event.data
    if event.event_type == "agent_started":
        return f"Agent '{data.get('name')}' started"
    elif event.event_type == "llm_call_started":
        return f"Calling {data.get('model')} ({data.get('message_count')} messages)"
    elif event.event_type == "llm_call_completed":
        return f"LLM responded in {data.get('duration_ms')}ms"
    elif event.event_type == "agent_completed":
        return f"Agent completed: {data.get('outcome')}"
    else:
        return str(event.event_type)


if __name__ == "__main__":
    asyncio.run(main())
