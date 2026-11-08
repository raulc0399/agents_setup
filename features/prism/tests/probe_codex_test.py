"""Regression tests for native evidence attribution and probe cleanup."""

import json
from pathlib import Path
import subprocess
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import AsyncMock, Mock, patch

import probe_codex as probe


def native_events():
    observation = {"meta_keys": ["threadId", "x-codex-turn-metadata"], "identity": {"threadId": "thread", "thread_id": "thread", "turn_id": "turn"}}
    item = {"type": "mcpToolCall", "server": "prism_probe", "tool": "inspect_caller", "status": "completed", "error": None,
            "result": {"content": [{"type": "text", "text": json.dumps(observation)}]}}
    return [
        {"id": 17, "method": "mcpServer/elicitation/request", "params": {"threadId": "thread", "turnId": "turn", "serverName": "prism_probe", "mode": "form", "_meta": {"codex_approval_kind": "mcp_tool_call", "tool_name": "inspect_caller", "tool_params": {}}}},
        {"method": "serverRequest/resolved", "params": {"threadId": "thread", "requestId": 17}},
        {"method": "item/completed", "params": {"threadId": "thread", "turnId": "turn", "item": item}},
        {"method": "item/completed", "params": {"threadId": "thread", "turnId": "turn", "item": {"type": "agentMessage", "text": "PROBE_DONE"}}},
        {"method": "turn/completed", "params": {"threadId": "thread", "turn": {"id": "turn", "status": "completed"}}},
    ]


class EvidenceTests(unittest.IsolatedAsyncioTestCase):
    async def run_model(self, events, pane="• PROBE_DONE\n"):
        first = SimpleNamespace(events=events, call=AsyncMock(return_value={"turn": {"id": "turn"}}))
        second = SimpleNamespace(events=events, call=AsyncMock(side_effect=[{"thread": {"id": "thread"}}, {"turnId": "turn"}]))
        args = SimpleNamespace(model="test-model", tui_approval=True, tui_switch=False, tui_command=False, codex="codex")
        report = {"checks": {}}
        with patch.object(probe.subprocess, "run", return_value=SimpleNamespace(stdout=pane, returncode=0)):
            await probe.model_probe(first, second, "thread", report, args, Path("/tmp/probe"), {"CODEX_HOME": "/tmp/probe/home"}, "test")
        return report["checks"]

    async def test_correlated_approval_and_assistant_reply_pass(self):
        checks = await self.run_model(native_events())
        self.assertEqual(checks["real_tui_mcp_approval"]["status"], "pass")

    async def test_prompt_echo_without_approval_does_not_pass(self):
        events = native_events()[2:3] + native_events()[4:]
        checks = await self.run_model(events, "› Call inspect_caller, then reply PROBE_DONE.\n")
        self.assertEqual(checks["real_tui_mcp_approval"]["status"], "fail")

    async def test_each_approval_evidence_component_is_required(self):
        for omitted in [0, 1, 3]:
            with self.subTest(omitted=omitted):
                events = native_events()
                del events[omitted]
                checks = await self.run_model(events)
                self.assertEqual(checks["real_tui_mcp_approval"]["status"], "fail")

    async def test_prompt_echo_does_not_count_as_rendered_assistant_reply(self):
        checks = await self.run_model(native_events(), "› Call inspect_caller, then reply PROBE_DONE.\n")
        self.assertEqual(checks["real_tui_mcp_approval"]["status"], "fail")

    async def test_unrelated_tool_results_do_not_prove_caller(self):
        for location, field, value in [("params", "threadId", "other"), ("params", "turnId", "other"), ("item", "server", "other"), ("item", "tool", "other")]:
            with self.subTest(field=field):
                events = native_events()
                target = events[2]["params"]
                if location == "item":
                    target = target["item"]
                target[field] = value
                checks = await self.run_model(events)
                self.assertEqual(checks["model_generated_mcp_caller"]["status"], "fail")

    async def test_native_turn_metadata_must_match(self):
        events = native_events()
        content = events[2]["params"]["item"]["result"]["content"][0]
        observation = json.loads(content["text"])
        observation["identity"]["turn_id"] = "other-turn"
        content["text"] = json.dumps(observation)
        checks = await self.run_model(events)
        self.assertEqual(checks["model_generated_mcp_caller"]["status"], "fail")


class LifecycleTests(unittest.IsolatedAsyncioTestCase):
    async def run_native(self, *, tui=False, close_error=False, resume_error=None, tmux_timeout=False):
        first = SimpleNamespace(close=AsyncMock(side_effect=RuntimeError("close failed") if close_error else None),
                                call=AsyncMock(side_effect=[{"process": {"id": 42}}, {"thread": {"id": "thread", "sessionId": "thread"}}, {"data": []}]))
        second = SimpleNamespace(close=AsyncMock(), call=AsyncMock(side_effect=[resume_error or RuntimeError("thread/resume: -32600: no rollout found for thread id thread"), {"thread": {"id": "other"}}, RuntimeError("turn/steer: -32600: no active turn to steer")]))
        server = Mock()
        args = SimpleNamespace(auth_file=None, codex="codex", model="test", model_turns=False, tui_approval=tui)
        report = {"checks": {}}
        with tempfile.TemporaryDirectory() as path:
            directory = Path(path)
            (directory / "server.sock").touch()
            initialized = {"codexHome": str(directory / "codex-home"), "userAgent": "probe/1"}
            with patch.object(probe, "install_probe_plugin", return_value=directory / "absent"), patch.object(probe, "connect", AsyncMock(side_effect=[(first, initialized), (second, initialized)])), patch.object(probe, "inspect_mcp", AsyncMock()), patch.object(probe.subprocess, "Popen", return_value=server), patch.object(probe.subprocess, "run", side_effect=subprocess.TimeoutExpired("tmux", 20) if tmux_timeout else None):
                try:
                    await probe.run_native(args, directory, report)
                except (RuntimeError, subprocess.TimeoutExpired):
                    pass
        return report, server, second

    async def test_client_close_failure_does_not_skip_server_cleanup(self):
        _, server, second = await self.run_native(close_error=True)
        server.terminate.assert_called_once()
        server.wait.assert_called()
        second.close.assert_awaited_once()

    async def test_tmux_timeout_does_not_skip_server_cleanup(self):
        _, server, second = await self.run_native(tui=True, tmux_timeout=True)
        server.terminate.assert_called_once()
        server.wait.assert_called()
        second.close.assert_awaited_once()

    async def test_expected_empty_thread_rejection_is_successful_negative_check(self):
        report, _, _ = await self.run_native()
        self.assertEqual(report["checks"]["empty_thread_resume"]["status"], "pass")

    async def test_unexpected_resume_error_fails(self):
        report, _, _ = await self.run_native(resume_error=RuntimeError("thread/resume: -32603: database unavailable"))
        self.assertEqual(report["checks"]["empty_thread_resume"]["status"], "fail")


if __name__ == "__main__":
    unittest.main()
