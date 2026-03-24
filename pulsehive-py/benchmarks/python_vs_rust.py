"""PulseHive Performance Comparison — Python bindings overhead measurement.

Measures the Python-side overhead of PulseHive operations to quantify
the cost of the PyO3 bridge. All measurements use a local PulseDB
substrate (no network I/O) and a mock LLM (which will error, but we
measure the framework overhead, not LLM latency).

Methodology:
- Each operation measured N times using time.perf_counter()
- Median and p99 reported (more stable than mean for I/O-bound ops)
- HiveMind build includes PulseDB initialization (disk I/O)
- Deploy includes agent spawn + event stream creation
- Event consumption measures async iterator overhead

Usage:
    cd /path/to/PulseHive
    source pulsehive-py/.venv/bin/activate
    python pulsehive-py/benchmarks/python_vs_rust.py
"""

import asyncio
import statistics
import tempfile
import time

from pulsehive import (
    AgentDefinition,
    AgentKind,
    HiveMind,
    Lens,
    LlmConfig,
    Task,
    openai_provider,
)

N_ITERATIONS = 10


def bench_hivemind_build():
    """Measure HiveMind construction time (includes PulseDB init)."""
    times = []
    for _ in range(N_ITERATIONS):
        with tempfile.TemporaryDirectory() as tmp:
            start = time.perf_counter()
            hive = (
                HiveMind.builder()
                .substrate_path(f"{tmp}/bench.db")
                .llm_provider("mock", openai_provider("sk-bench", "gpt-4"))
                .build()
            )
            elapsed = time.perf_counter() - start
            times.append(elapsed * 1000)  # ms
            hive.shutdown()
    return times


async def bench_deploy_overhead():
    """Measure time from deploy() call to first event."""
    times = []
    for _ in range(N_ITERATIONS):
        with tempfile.TemporaryDirectory() as tmp:
            hive = (
                HiveMind.builder()
                .substrate_path(f"{tmp}/bench.db")
                .llm_provider("mock", openai_provider("sk-bench", "gpt-4"))
                .build()
            )
            agent = AgentDefinition(
                "bench-agent",
                AgentKind.llm("You are a benchmark.", Lens([]), LlmConfig("mock", "gpt-4")),
            )

            start = time.perf_counter()
            stream = await hive.deploy([agent], [Task("benchmark")])
            # Time to first event
            async for event in stream:
                elapsed = time.perf_counter() - start
                times.append(elapsed * 1000)
                break
            # Drain remaining
            async for event in stream:
                if event.event_type == "agent_completed":
                    break
            hive.shutdown()
    return times


async def bench_event_consumption():
    """Measure event stream consumption throughput."""
    times = []
    for _ in range(N_ITERATIONS):
        with tempfile.TemporaryDirectory() as tmp:
            hive = (
                HiveMind.builder()
                .substrate_path(f"{tmp}/bench.db")
                .llm_provider("mock", openai_provider("sk-bench", "gpt-4"))
                .build()
            )
            agent = AgentDefinition(
                "bench",
                AgentKind.llm("Benchmark.", Lens([]), LlmConfig("mock", "gpt-4")),
            )

            stream = await hive.deploy([agent], [Task("bench")])
            count = 0
            start = time.perf_counter()
            async for event in stream:
                count += 1
                if event.event_type == "agent_completed":
                    break
            elapsed = time.perf_counter() - start
            if count > 0:
                times.append((elapsed * 1000) / count)  # ms per event
            hive.shutdown()
    return times


def bench_tool_bridge():
    """Measure Python tool construction overhead (PythonToolBridge creation)."""

    class BenchTool:
        def name(self): return "bench"
        def description(self): return "Benchmark tool"
        def parameters(self): return {"type": "object"}
        def execute(self, params, context): return "ok"

    times = []
    for _ in range(N_ITERATIONS * 10):  # More iterations for micro-benchmark
        tool = BenchTool()
        start = time.perf_counter()
        kind = AgentKind.llm("test", Lens([]), LlmConfig("m", "m"), tools=[tool])
        elapsed = time.perf_counter() - start
        times.append(elapsed * 1000)
    return times


def fmt(times):
    """Format timing stats."""
    if not times:
        return "N/A"
    med = statistics.median(times)
    p99 = sorted(times)[int(len(times) * 0.99)] if len(times) > 1 else times[0]
    return f"{med:.2f}ms (p99: {p99:.2f}ms)"


async def main():
    print("PulseHive Python Bindings — Performance Benchmark")
    print("=" * 55)
    print(f"Iterations: {N_ITERATIONS} per test\n")

    print("1. HiveMind build (includes PulseDB init)...")
    build_times = bench_hivemind_build()

    print("2. Deploy overhead (time to first event)...")
    deploy_times = await bench_deploy_overhead()

    print("3. Event consumption (ms per event)...")
    event_times = await bench_event_consumption()

    print("4. Tool bridge construction...")
    tool_times = bench_tool_bridge()

    print("\n" + "=" * 55)
    print("Results")
    print("=" * 55)
    print(f"{'Operation':<35} {'Python (PyO3)':<20}")
    print("-" * 55)
    print(f"{'HiveMind.builder().build()':<35} {fmt(build_times):<20}")
    print(f"{'deploy() → first event':<35} {fmt(deploy_times):<20}")
    print(f"{'Event consumption (per event)':<35} {fmt(event_times):<20}")
    print(f"{'Tool bridge construction':<35} {fmt(tool_times):<20}")
    print("-" * 55)
    print("\nNote: LLM calls use a mock provider (errors expected).")
    print("These numbers measure framework + PyO3 bridge overhead only.")


if __name__ == "__main__":
    asyncio.run(main())
