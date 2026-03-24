"""PulseHive Custom Tools — Define tools in Python using duck-typing.

This example demonstrates:
1. Defining tools as plain Python classes (no base class needed)
2. The tool protocol: name(), description(), parameters(), execute()
3. ToolContext access (agent_id, collective_id)
4. Error handling in tools
5. Optional requires_approval() method

Prerequisites:
    pip install pulsehive
    export OPENAI_API_KEY="sk-..."

Usage:
    python examples/custom_tools.py
"""

import asyncio
import json
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


# ── Tool 1: Calculator ────────────────────────────────────────────────
# A simple tool that performs arithmetic operations.

class CalculatorTool:
    """Basic calculator — add, subtract, multiply, divide."""

    def name(self):
        return "calculator"

    def description(self):
        return "Performs basic arithmetic operations: add, subtract, multiply, divide"

    def parameters(self):
        return {
            "type": "object",
            "properties": {
                "operation": {
                    "type": "string",
                    "enum": ["add", "subtract", "multiply", "divide"],
                    "description": "The arithmetic operation to perform",
                },
                "a": {"type": "number", "description": "First operand"},
                "b": {"type": "number", "description": "Second operand"},
            },
            "required": ["operation", "a", "b"],
        }

    def execute(self, params, context):
        """Execute the calculation. Returns a string result."""
        op = params["operation"]
        a = params["a"]
        b = params["b"]

        print(f"    [Calculator] {a} {op} {b} (agent: {context.agent_id[:8]})")

        if op == "add":
            return str(a + b)
        elif op == "subtract":
            return str(a - b)
        elif op == "multiply":
            return str(a * b)
        elif op == "divide":
            if b == 0:
                return "Error: division by zero"
            return str(a / b)
        else:
            return f"Error: unknown operation '{op}'"


# ── Tool 2: Word Counter ─────────────────────────────────────────────
# Returns a dict (JSON result) instead of a string.

class WordCounterTool:
    """Counts words, characters, and sentences in text."""

    def name(self):
        return "word_counter"

    def description(self):
        return "Analyzes text and returns word count, character count, and sentence count"

    def parameters(self):
        return {
            "type": "object",
            "properties": {
                "text": {"type": "string", "description": "The text to analyze"},
            },
            "required": ["text"],
        }

    def execute(self, params, context):
        """Execute analysis. Returns a dict (converted to JSON result)."""
        text = params["text"]
        return {
            "words": len(text.split()),
            "characters": len(text),
            "sentences": text.count(".") + text.count("!") + text.count("?"),
        }


# ── Tool 3: Approval-Required Tool ───────────────────────────────────
# Demonstrates the optional requires_approval() method.

class DatabaseWriteTool:
    """Simulates a database write that requires human approval."""

    def name(self):
        return "database_write"

    def description(self):
        return "Writes data to the database (requires approval)"

    def parameters(self):
        return {
            "type": "object",
            "properties": {
                "table": {"type": "string"},
                "data": {"type": "object"},
            },
            "required": ["table", "data"],
        }

    def requires_approval(self):
        """This tool requires human approval before execution."""
        return True

    def execute(self, params, context):
        return f"Wrote to {params['table']}: {json.dumps(params.get('data', {}))}"


# ── Main ──────────────────────────────────────────────────────────────

async def main():
    api_key = os.environ.get("OPENAI_API_KEY", "")
    if not api_key:
        print("Set OPENAI_API_KEY to run with a real LLM.")
        print("Running with empty key (agent will error, but tools are registered).\n")

    hive = (
        HiveMind.builder()
        .substrate_path("/tmp/pulsehive_tools.db")
        .llm_provider("openai", openai_provider(api_key, "gpt-4"))
        .build()
    )

    # Create agent with all three tools
    agent = AgentDefinition(
        "tool-user",
        AgentKind.llm(
            system_prompt=(
                "You are a helpful assistant with access to tools. "
                "Use the calculator for math, word_counter for text analysis."
            ),
            lens=Lens(["tools"]),
            llm_config=LlmConfig("openai", "gpt-4"),
            tools=[
                CalculatorTool(),
                WordCounterTool(),
                DatabaseWriteTool(),  # Requires approval (will be auto-approved in default config)
            ],
        ),
    )

    print(f"Agent '{agent.name}' with {agent.kind_tag} kind")
    print("Tools: calculator, word_counter, database_write\n")

    # Deploy
    stream = await hive.deploy([agent], [Task("Calculate 42 * 7, then count the words in 'Hello world!'")])

    async for event in stream:
        data = event.data
        if event.event_type == "tool_call_started":
            print(f"  -> Tool called: {data.get('tool_name')}")
        elif event.event_type == "tool_call_completed":
            print(f"  <- Tool done: {data.get('tool_name')} ({data.get('duration_ms')}ms)")
        elif event.event_type == "tool_approval_requested":
            print(f"  !! Approval requested for: {data.get('tool_name')}")
        elif event.event_type == "agent_completed":
            outcome = data.get("outcome", "unknown")
            if outcome == "complete":
                print(f"\nResult: {data.get('response', '')[:300]}")
            else:
                print(f"\nAgent finished: {outcome}")
            break

    hive.shutdown()
    print("\nDone!")


if __name__ == "__main__":
    asyncio.run(main())
