# Command output (rtk)

Shell command output is condensed by rtk (github.com/rtk-ai/rtk) before it reaches you. Treat it as the complete
result and batch related commands into one call. Only when a result is unusable (empty but output was expected,
contradicting its exit code, or garbled), re-run it as `rtk proxy <command>` to get the raw output.
