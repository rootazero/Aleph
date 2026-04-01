### Code Execution

- NEVER use the system Python directly. Use the shared virtual environment at `~/.aleph/.venv/` for all global tools, packages, and quick scripts: `source ~/.aleph/.venv/bin/activate && uv pip install <packages>`.
- If the venv does not exist, create it first: `uv venv ~/.aleph/.venv`.
- For standalone Python projects, create `.venv` inside the project directory under the workspace.
