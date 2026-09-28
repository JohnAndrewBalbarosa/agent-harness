import os, sys
from pathlib import Path
Path(os.environ["FAKE_OUT"]).write_bytes(sys.stdin.buffer.read())
