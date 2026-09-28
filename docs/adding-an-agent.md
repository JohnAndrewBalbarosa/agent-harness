# Adding an agent

An agent joins agent-harness through one directory, `adapters/<name>/`, and one line in the conformance kit.
No consumer, router or compile code changes are needed when the agent's hook payloads use the common snake_case
fields (`hook_event_name`, `session_id`, `cwd`, `transcript_path`, `prompt`, `tool_name`, `tool_input`,
`tool_response`, `last_assistant_message`, `agent_id`, `agent_type`), as Codex CLI and Claude Code do.

## Checklist

1. **Research first.** Read the agent's official hook and telemetry docs; record the technology in the technology
   memory. Capture one real payload per hook event (sanitize prompts, ids and paths).
2. **`adapters/<name>/manifest.toml`**
   ```toml
   agent = "<name>"
   rulesync_target = "<rulesync target id>"      # see `npx rulesync generate --help`
   events = ["SessionStart", "UserPromptSubmit", "PostToolUse", "Stop"]   # native event names you wire
   tested_versions = ["<agent --version>"]
   otel_service_names = ["<service.name prefix>"] # omit if the agent has no OpenTelemetry
   ```
3. **`adapters/<name>/adapter.py`** exposing `ADAPTER` with `name`, `instance(env)`, `normalize(native, payload, env)`,
   `respond(event, results)`, `safe_default(native)`. Start from `adapters/base.py:SHARED_MAPPING` and add only the
   agent's extra events. `normalize` returns `None` for anything unknown; `safe_default` is what the agent receives
   when anything fails.
4. **Fixtures** in `adapters/<name>/fixtures/<NativeEvent>.json`, one per wired event.
5. **Optional:** `otel.py` (`map(attrs) -> dict | None`, whitelist only), `transcript.py`
   (`usage(path, turn_id) -> TurnUsage`), `background.py` (`background_shells(path, now)`), `patches.py`
   (native config the rulesync hooks do not cover; wire it in `compile/run.py`).
6. **Conformance:** add the name to `ADAPTERS` in `tests/conformance/test_adapter_contract.py` and run
   `python -m unittest discover -s tests -t .`.
7. **Configure:** add `<name>:<instance>=<home>` to `INSTANCES` in `.env`, run `python -m compile.run --dry-run`,
   review, then apply and start a fresh agent session to verify.
