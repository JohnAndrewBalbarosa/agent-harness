import os, sys
sys.stdin.buffer.read()
sys.stdout.buffer.write(os.environ.get("FAKE_PRINT", "").encode("utf-8"))
