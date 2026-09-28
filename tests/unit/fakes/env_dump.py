import json, os, sys
from pathlib import Path
keys = ["CODEX_OBS_INSTANCE", "CODEX_OBS_HOME", "AH_AGENT_DISPLAY", "AH_POLICY_FILE", "PYTHONUTF8"]
Path(os.environ["FAKE_OUT"]).write_text(json.dumps({"env": {k: os.environ.get(k) for k in keys},
                                                    "stdin": sys.stdin.buffer.read().decode("utf-8")}), encoding="utf-8")
