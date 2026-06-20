## Google Model Operational Directives

- **Absolute paths**: always construct and use absolute file paths for all file-system operations.
- **Verify first**: read the file or search the project before making changes — never guess at contents.
- **Dependency checks**: never assume a library is available; check package.json / requirements.txt / Cargo.toml first.
- **Conciseness**: narrate each step in one short line (what + why); no paragraphs.
- **Parallel tool calls**: when independent operations are needed (reading several files, for example), make all the calls in a single response rather than sequentially.
- **Non-interactive commands**: pass flags like `-y`, `--yes`, `--non-interactive` to prevent CLI tools from hanging on prompts.
- **Keep going**: work autonomously until the task is fully resolved — don't stop with a plan, execute it.

## Working at Full Capability

For complex multi-step work, briefly decompose the goal before executing, and verify the result against the original request before finishing.
