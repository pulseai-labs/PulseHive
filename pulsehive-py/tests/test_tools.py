"""Unit tests for Python tool bindings — ToolContext, ToolResult, and tool protocol."""

import pytest

from pulsehive import (
    AgentKind,
    AgentDefinition,
    Lens,
    LlmConfig,
    ToolContext,
    ToolResult,
)


# ── Helper tool classes ───────────────────────────────────────────────

class EchoTool:
    """Minimal valid tool."""
    def name(self): return "echo"
    def description(self): return "Echoes input text"
    def parameters(self): return {"type": "object", "properties": {"text": {"type": "string"}}}
    def execute(self, params, context): return f"Echo: {params.get('text', 'nothing')}"


class DictReturnTool:
    """Tool that returns a dict (JSON result)."""
    def name(self): return "lookup"
    def description(self): return "Looks up a value"
    def parameters(self): return {"type": "object", "properties": {"key": {"type": "string"}}}
    def execute(self, params, context): return {"result": params.get("key", ""), "found": True}


class NoneReturnTool:
    """Tool that returns None."""
    def name(self): return "void"
    def description(self): return "Returns nothing"
    def parameters(self): return {"type": "object"}
    def execute(self, params, context): return None


class ExceptionTool:
    """Tool that raises an exception."""
    def name(self): return "fail"
    def description(self): return "Always fails"
    def parameters(self): return {"type": "object"}
    def execute(self, params, context): raise ValueError("Intentional failure")


class ApprovalRequiredTool:
    """Tool that requires approval."""
    def name(self): return "dangerous"
    def description(self): return "Requires approval"
    def parameters(self): return {"type": "object"}
    def requires_approval(self): return True
    def execute(self, params, context): return "Executed"


# ── ToolResult tests ──────────────────────────────────────────────────

class TestToolResult:
    def test_text_factory(self):
        r = ToolResult.text("hello")
        assert r.kind == "text"
        assert r.content == "hello"

    def test_error_factory(self):
        r = ToolResult.error("oops")
        assert r.kind == "error"
        assert r.content == "oops"

    def test_repr_text(self):
        r = ToolResult.text("short")
        assert "text" in repr(r)
        assert "short" in repr(r)

    def test_repr_error(self):
        r = ToolResult.error("broken")
        assert "error" in repr(r)

    def test_repr_truncates_long_content(self):
        r = ToolResult.text("x" * 100)
        rep = repr(r)
        assert "..." in rep


# ── Tool protocol validation tests ────────────────────────────────────

class TestToolProtocolValidation:
    def _make_agent(self, tools):
        lens = Lens([])
        cfg = LlmConfig("mock", "test")
        return AgentKind.llm("test prompt", lens, cfg, tools=tools)

    def test_valid_tool_accepted(self):
        """EchoTool has all required methods."""
        kind = self._make_agent([EchoTool()])
        assert kind.kind_tag == "llm"

    def test_multiple_tools_accepted(self):
        kind = self._make_agent([EchoTool(), DictReturnTool()])
        assert kind.kind_tag == "llm"

    def test_missing_name_raises_type_error(self):
        class NoName:
            def description(self): return "x"
            def parameters(self): return {}
            def execute(self, p, c): return "x"

        with pytest.raises(TypeError, match="name"):
            self._make_agent([NoName()])

    def test_missing_description_raises_type_error(self):
        class NoDesc:
            def name(self): return "x"
            def parameters(self): return {}
            def execute(self, p, c): return "x"

        with pytest.raises(TypeError, match="description"):
            self._make_agent([NoDesc()])

    def test_missing_parameters_raises_type_error(self):
        class NoParams:
            def name(self): return "x"
            def description(self): return "x"
            def execute(self, p, c): return "x"

        with pytest.raises(TypeError, match="parameters"):
            self._make_agent([NoParams()])

    def test_missing_execute_raises_type_error(self):
        class NoExec:
            def name(self): return "x"
            def description(self): return "x"
            def parameters(self): return {}

        with pytest.raises(TypeError, match="execute"):
            self._make_agent([NoExec()])

    def test_none_tools_is_backward_compatible(self):
        """tools=None should work (empty tools list)."""
        lens = Lens([])
        cfg = LlmConfig("mock", "test")
        kind = AgentKind.llm("prompt", lens, cfg, tools=None)
        assert kind.kind_tag == "llm"

    def test_empty_tools_list(self):
        """tools=[] should work (empty tools list)."""
        lens = Lens([])
        cfg = LlmConfig("mock", "test")
        kind = AgentKind.llm("prompt", lens, cfg, tools=[])
        assert kind.kind_tag == "llm"

    def test_approval_required_tool(self):
        """Tool with requires_approval() should be accepted."""
        kind = self._make_agent([ApprovalRequiredTool()])
        assert kind.kind_tag == "llm"

    def test_tool_with_agent_definition(self):
        """Full agent creation with tools."""
        kind = self._make_agent([EchoTool(), DictReturnTool()])
        agent = AgentDefinition("tool-agent", kind)
        assert agent.name == "tool-agent"
        assert agent.kind_tag == "llm"
