"""Unit tests for PulseHive Python type bindings."""

from pulsehive import (
    LlmConfig,
    Lens,
    RecencyCurve,
    AgentKind,
    AgentDefinition,
    HiveMind,
    Task,
    openai_provider,
    anthropic_provider,
    version,
)


class TestVersion:
    def test_version_returns_string(self):
        v = version()
        assert isinstance(v, str)
        assert v == "0.1.0"


class TestLlmConfig:
    def test_defaults(self):
        cfg = LlmConfig("openai", "gpt-4")
        assert cfg.provider == "openai"
        assert cfg.model == "gpt-4"
        assert abs(cfg.temperature - 0.7) < 0.01
        assert cfg.max_tokens == 4096

    def test_custom_params(self):
        cfg = LlmConfig("anthropic", "claude-sonnet-4-6", temperature=0.3, max_tokens=8192)
        assert cfg.provider == "anthropic"
        assert abs(cfg.temperature - 0.3) < 0.01
        assert cfg.max_tokens == 8192

    def test_repr(self):
        cfg = LlmConfig("openai", "gpt-4")
        r = repr(cfg)
        assert "openai" in r
        assert "gpt-4" in r


class TestRecencyCurve:
    def test_exponential(self):
        rc = RecencyCurve.exponential(48.0)
        assert "exponential" in repr(rc)

    def test_uniform(self):
        rc = RecencyCurve.uniform()
        assert "uniform" in repr(rc)


class TestLens:
    def test_basic(self):
        lens = Lens(["safety", "clinical"])
        assert lens.domain_focus == ["safety", "clinical"]
        assert lens.attention_budget == 50

    def test_custom_budget(self):
        lens = Lens(["code"], attention_budget=100)
        assert lens.attention_budget == 100

    def test_with_recency_curve(self):
        lens = Lens(["test"], recency_curve=RecencyCurve.uniform())
        assert "uniform" in repr(lens)

    def test_with_type_weights(self):
        lens = Lens(["test"], type_weights={"error_pattern": 3.0, "fact": 0.5})
        weights = lens.type_weights
        assert isinstance(weights, dict)
        assert len(weights) == 2

    def test_repr(self):
        lens = Lens(["safety"])
        r = repr(lens)
        assert "safety" in r


class TestAgentKind:
    def test_llm(self):
        lens = Lens(["test"])
        cfg = LlmConfig("openai", "gpt-4")
        kind = AgentKind.llm("You are helpful.", lens, cfg)
        assert kind.kind_tag == "llm"
        assert "llm" in repr(kind)

    def test_sequential(self):
        a1 = AgentDefinition("a", AgentKind.llm("prompt", Lens([]), LlmConfig("o", "m")))
        a2 = AgentDefinition("b", AgentKind.llm("prompt", Lens([]), LlmConfig("o", "m")))
        kind = AgentKind.sequential([a1, a2])
        assert kind.kind_tag == "sequential"
        assert "2 agents" in repr(kind)

    def test_parallel(self):
        a1 = AgentDefinition("a", AgentKind.llm("prompt", Lens([]), LlmConfig("o", "m")))
        kind = AgentKind.parallel([a1])
        assert kind.kind_tag == "parallel"

    def test_loop(self):
        a = AgentDefinition("a", AgentKind.llm("prompt", Lens([]), LlmConfig("o", "m")))
        kind = AgentKind.loop_(a, 5)
        assert kind.kind_tag == "loop"
        assert "5" in repr(kind)

    def test_nested_workflows(self):
        """Sequential containing parallel — tests recursive nesting."""
        inner_a = AgentDefinition("i1", AgentKind.llm("p", Lens([]), LlmConfig("o", "m")))
        inner_b = AgentDefinition("i2", AgentKind.llm("p", Lens([]), LlmConfig("o", "m")))
        par = AgentDefinition("par", AgentKind.parallel([inner_a, inner_b]))
        summary = AgentDefinition("sum", AgentKind.llm("summarize", Lens([]), LlmConfig("o", "m")))
        seq = AgentDefinition("pipeline", AgentKind.sequential([par, summary]))
        assert seq.kind_tag == "sequential"


class TestAgentDefinition:
    def test_construction(self):
        kind = AgentKind.llm("prompt", Lens([]), LlmConfig("o", "m"))
        agent = AgentDefinition("researcher", kind)
        assert agent.name == "researcher"
        assert agent.kind_tag == "llm"

    def test_repr(self):
        kind = AgentKind.llm("prompt", Lens([]), LlmConfig("o", "m"))
        agent = AgentDefinition("test", kind)
        r = repr(agent)
        assert "test" in r
        assert "llm" in r


class TestTask:
    def test_construction(self):
        task = Task("Analyze the codebase")
        assert task.description == "Analyze the codebase"

    def test_repr(self):
        task = Task("hello")
        assert "hello" in repr(task)


class TestHiveMindBuilder:
    def test_build_without_substrate_raises(self):
        import pytest
        with pytest.raises(RuntimeError, match="Substrate not configured"):
            HiveMind.builder().build()

    def test_build_with_substrate(self, tmp_path):
        hive = HiveMind.builder().substrate_path(str(tmp_path / "test.db")).build()
        assert repr(hive) == "HiveMind(active)"

    def test_shutdown(self, tmp_path):
        hive = HiveMind.builder().substrate_path(str(tmp_path / "test.db")).build()
        assert not hive.is_shutdown()
        hive.shutdown()
        assert hive.is_shutdown()


class TestProviderFactories:
    def test_openai_provider(self):
        p = openai_provider("sk-test", "gpt-4")
        assert "openai" in repr(p)

    def test_anthropic_provider(self):
        p = anthropic_provider("sk-ant-test")
        assert "anthropic" in repr(p)

    def test_register_provider(self, tmp_path):
        p = openai_provider("sk-test", "gpt-4")
        hive = (
            HiveMind.builder()
            .substrate_path(str(tmp_path / "test.db"))
            .llm_provider("openai", p)
            .build()
        )
        assert repr(hive) == "HiveMind(active)"
