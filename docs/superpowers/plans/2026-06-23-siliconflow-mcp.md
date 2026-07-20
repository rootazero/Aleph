# SiliconFlow MCP Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build `aleph-siliconflow-mcp`, a Python MCP server exposing SiliconFlow media generation (image / video / TTS) as tools, in a new `rootazero/Aleph-mcp` monorepo, and wire it into Aleph's in-binary MCP preset catalog.

**Architecture:** A standalone Python process (official `mcp` SDK / FastMCP) that talks to the SiliconFlow REST API over `httpx`, saves generated assets locally, and returns LLM-readable text. It is installed on demand via `uvx --from git+…#subdirectory=siliconflow`. Aleph's only change is one entry in `src/mcp/presets/catalog.json` plus a test-id assertion. Pure functions (payload building, response parsing, ratio mapping, filename derivation) are separated from IO so they unit-test with zero network.

**Tech Stack:** Python ≥ 3.10, `mcp` (FastMCP), `httpx`, `python-dotenv`, `pytest`, `uv`/`uvx`. Aleph side: Rust (`alephcore`), JSON catalog.

## Global Constraints

- **Owner / repo:** `rootazero/Aleph-mcp`, public, created fresh (no history), via `gh repo create … --source=. --push`.
- **Package name:** `aleph-siliconflow-mcp`; console script `aleph-siliconflow-mcp = "aleph_siliconflow_mcp.main:main"`.
- **Repo layout:** monorepo container; this server lives under `siliconflow/` (so `uvx --from git+…#subdirectory=siliconflow`).
- **API base default:** `https://api.siliconflow.cn/v1` (cn-native), overridable via `SILICONFLOW_API_BASE`. Auth: `Authorization: Bearer <SILICONFLOW_API_KEY>`.
- **Scope:** media-only (image/video/TTS) + `list_models` + `get_user_info`. NEVER add chat / embedding / rerank (Aleph core already covers them).
- **Model names:** never hardcode an authoritative model list; provide sensible defaults + rely on `list_models` for discovery.
- **Endpoints (verified vs official docs 2026-06-23):** image gen+edit share `POST /v1/images/generations`; video `POST /v1/video/submit` then poll `POST /v1/video/status` (status value `Succeed`, urls at `results.videos[].url`); TTS `POST /v1/audio/speech` (binary; voice is `model:voice_id`); `GET /v1/models`; `GET /v1/user/info`.
- **Python style:** `from __future__ import annotations` where needed; `str | None` hints; no import-time env reads (lazy `get_client()`); every tool function has a one-line docstring (FastMCP turns it into the tool description).
- **All work in `/Volumes/TBU4/Workspace/Aleph-mcp`** except Task 10 (main repo `/Volumes/TBU4/Workspace/Aleph`). Run commands with `git -C <dir>` / `uv run --directory <dir>` to avoid `cd`.

---

## File Structure

```
/Volumes/TBU4/Workspace/Aleph-mcp/
├── README.md                       # repo charter + server index + credits (Task 8)
├── LICENSE                         # MIT (Task 1)
├── .gitignore                      # Python (Task 1)
└── siliconflow/
    ├── pyproject.toml              # package + script entry (Task 1)
    ├── README.md                   # install/config/tools (Task 8)
    ├── .env.example                # (Task 8)
    ├── src/aleph_siliconflow_mcp/
    │   ├── __init__.py             # (Task 1)
    │   ├── ratios.py               # pure aspect_ratio→size maps (Task 1)
    │   ├── client.py               # Settings, client, IO, render (Task 2)
    │   ├── images.py               # generate_image / edit_image (Task 3)
    │   ├── videos.py               # submit / status / generate (Task 4)
    │   ├── audio.py                # generate_speech (Task 5)
    │   ├── user.py                 # get_user_info / list_models (Task 6)
    │   ├── server.py               # FastMCP registration (Task 7)
    │   └── main.py                 # entry point (Task 7)
    └── tests/
        ├── test_ratios.py          # (Task 1)
        ├── test_client.py          # (Task 2)
        ├── test_images.py          # (Task 3)
        ├── test_videos.py          # (Task 4)
        ├── test_audio.py           # (Task 5)
        └── test_user.py            # (Task 6)

/Volumes/TBU4/Workspace/Aleph/
├── src/mcp/presets/catalog.json    # +1 preset (Task 10)
└── src/mcp/presets/mod.rs          # +1 test id assertion (Task 10)
```

---

### Task 1: Scaffold the package + ratios module

**Files:**
- Create: `siliconflow/pyproject.toml`, `siliconflow/.env.example` (stub), `LICENSE`, `.gitignore`
- Create: `siliconflow/src/aleph_siliconflow_mcp/__init__.py`
- Create: `siliconflow/src/aleph_siliconflow_mcp/ratios.py`
- Test: `siliconflow/tests/test_ratios.py`

**Interfaces:**
- Produces: `ratios.image_size_for(aspect_ratio: str) -> str`, `ratios.video_size_for(aspect_ratio: str) -> str` (raise `ValueError` on unknown ratio). Constants `ratios.IMAGE_SIZES`, `ratios.VIDEO_SIZES`.

- [ ] **Step 1: Create the directory tree and git init**

```bash
mkdir -p /Volumes/TBU4/Workspace/Aleph-mcp/siliconflow/src/aleph_siliconflow_mcp
mkdir -p /Volumes/TBU4/Workspace/Aleph-mcp/siliconflow/tests
git -C /Volumes/TBU4/Workspace/Aleph-mcp init -q
```

- [ ] **Step 2: Write `LICENSE` (MIT) and `.gitignore`**

`/Volumes/TBU4/Workspace/Aleph-mcp/.gitignore`:
```
__pycache__/
*.py[cod]
.venv/
*.egg-info/
.pytest_cache/
.env
dist/
build/
uv.lock
```

`/Volumes/TBU4/Workspace/Aleph-mcp/LICENSE`: standard MIT License text, copyright line: `Copyright (c) 2026 Aleph (rootazero)`.

- [ ] **Step 3: Write `siliconflow/pyproject.toml`**

```toml
[project]
name = "aleph-siliconflow-mcp"
version = "0.1.0"
description = "Aleph's official SiliconFlow media-generation MCP server (image / video / TTS)"
readme = "README.md"
requires-python = ">=3.10"
keywords = ["mcp", "siliconflow", "image", "video", "tts", "aleph"]
dependencies = [
    "httpx>=0.28.1",
    "mcp>=1.27.0",
    "python-dotenv>=1.0.0",
]

[project.scripts]
aleph-siliconflow-mcp = "aleph_siliconflow_mcp.main:main"

[project.optional-dependencies]
dev = ["pytest>=8.0.0"]

[build-system]
requires = ["hatchling"]
build-backend = "hatchling.build"

[tool.hatch.build.targets.wheel]
packages = ["src/aleph_siliconflow_mcp"]
```

- [ ] **Step 4: Write `__init__.py` and the failing test**

`siliconflow/src/aleph_siliconflow_mcp/__init__.py`:
```python
"""Aleph SiliconFlow MCP server."""

__version__ = "0.1.0"
```

`siliconflow/tests/test_ratios.py`:
```python
import pytest

from aleph_siliconflow_mcp.ratios import image_size_for, video_size_for


def test_image_size_known_ratios():
    assert image_size_for("1:1") == "1024x1024"
    assert image_size_for("16:9") == "1024x576"
    assert image_size_for("9:16") == "576x1024"


def test_image_size_unknown_raises():
    with pytest.raises(ValueError, match="aspect_ratio"):
        image_size_for("21:9")


def test_video_size_known_ratios():
    assert video_size_for("16:9") == "1280x720"
    assert video_size_for("9:16") == "720x1280"
    assert video_size_for("1:1") == "960x960"


def test_video_size_unknown_raises():
    with pytest.raises(ValueError, match="aspect_ratio"):
        video_size_for("4:3")
```

- [ ] **Step 5: Run the test to verify it fails**

Run: `uv run --directory /Volumes/TBU4/Workspace/Aleph-mcp/siliconflow --extra dev pytest tests/test_ratios.py -v`
Expected: FAIL — `ModuleNotFoundError: No module named 'aleph_siliconflow_mcp.ratios'`

- [ ] **Step 6: Implement `ratios.py`**

`siliconflow/src/aleph_siliconflow_mcp/ratios.py`:
```python
"""Pure aspect-ratio → pixel-size mappings (no IO)."""

IMAGE_SIZES = {
    "1:1": "1024x1024",
    "3:4": "768x1024",
    "4:3": "1024x768",
    "9:16": "576x1024",
    "16:9": "1024x576",
}

# Official video API accepts only these three sizes.
VIDEO_SIZES = {
    "16:9": "1280x720",
    "9:16": "720x1280",
    "1:1": "960x960",
}


def image_size_for(aspect_ratio: str) -> str:
    try:
        return IMAGE_SIZES[aspect_ratio]
    except KeyError:
        allowed = ", ".join(IMAGE_SIZES)
        raise ValueError(
            f"unsupported image aspect_ratio '{aspect_ratio}'; allowed: {allowed}"
        )


def video_size_for(aspect_ratio: str) -> str:
    try:
        return VIDEO_SIZES[aspect_ratio]
    except KeyError:
        allowed = ", ".join(VIDEO_SIZES)
        raise ValueError(
            f"unsupported video aspect_ratio '{aspect_ratio}'; allowed: {allowed}"
        )
```

- [ ] **Step 7: Run the test to verify it passes**

Run: `uv run --directory /Volumes/TBU4/Workspace/Aleph-mcp/siliconflow --extra dev pytest tests/test_ratios.py -v`
Expected: PASS (4 passed)

- [ ] **Step 8: Commit**

```bash
git -C /Volumes/TBU4/Workspace/Aleph-mcp add -A
git -C /Volumes/TBU4/Workspace/Aleph-mcp commit -q -m "feat: scaffold aleph-siliconflow-mcp + ratio mappings"
```

---

### Task 2: Client — config, HTTP, errors, asset saving, rendering

**Files:**
- Create: `siliconflow/src/aleph_siliconflow_mcp/client.py`
- Test: `siliconflow/tests/test_client.py`

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces:
  - `SiliconFlowError(Exception)`
  - `Settings` dataclass: `api_key, api_base, image_dir, audio_dir`; classmethod `from_env() -> Settings`
  - `extract_api_error(status_code: int, body: str) -> str`
  - `ext_from_url(url: str, default: str = ".png") -> str`
  - `build_filename(prefix: str, ext: str, stamp: int) -> str`
  - `looks_remote(value: str) -> bool`
  - `to_image_field(value: str) -> str` (URL/data-URI passthrough; local path → `data:image/...;base64,...`; missing file → `SiliconFlowError`)
  - `class SiliconFlowClient(settings)`: `settings`; `async request_json(method, path, *, json=None, params=None) -> dict`; `async request_binary(method, path, *, json=None) -> bytes`; `async download(url, save_dir, prefix) -> str`; `save_binary(content, save_dir, ext, prefix) -> str`
  - `get_client() -> SiliconFlowClient` (lazy singleton from env; raises `SiliconFlowError` if key missing)
  - `async render_assets(client, kind, urls, save_dir, prefix, header) -> str`

- [ ] **Step 1: Write the failing test**

`siliconflow/tests/test_client.py`:
```python
import base64

import pytest

from aleph_siliconflow_mcp.client import (
    Settings,
    SiliconFlowError,
    build_filename,
    extract_api_error,
    ext_from_url,
    looks_remote,
    to_image_field,
)


def test_settings_from_env_defaults(monkeypatch):
    monkeypatch.setenv("SILICONFLOW_API_KEY", "  sk-abc ")
    monkeypatch.delenv("SILICONFLOW_API_BASE", raising=False)
    monkeypatch.delenv("SILICONFLOW_IMAGE_DIR", raising=False)
    monkeypatch.delenv("SILICONFLOW_AUDIO_DIR", raising=False)
    s = Settings.from_env()
    assert s.api_key == "sk-abc"
    assert s.api_base == "https://api.siliconflow.cn/v1"
    assert s.image_dir is None
    assert s.audio_dir is None


def test_settings_audio_dir_falls_back_to_image_dir(monkeypatch):
    monkeypatch.setenv("SILICONFLOW_API_KEY", "k")
    monkeypatch.setenv("SILICONFLOW_IMAGE_DIR", "/tmp/imgs")
    monkeypatch.delenv("SILICONFLOW_AUDIO_DIR", raising=False)
    monkeypatch.setenv("SILICONFLOW_API_BASE", "https://api.siliconflow.com/v1/")
    s = Settings.from_env()
    assert s.audio_dir == "/tmp/imgs"
    assert s.api_base == "https://api.siliconflow.com/v1"  # trailing slash stripped


def test_extract_api_error_json_message():
    body = '{"message": "invalid model"}'
    assert extract_api_error(400, body) == "SiliconFlow API error 400: invalid model"


def test_extract_api_error_plain_text():
    assert extract_api_error(500, "boom") == "SiliconFlow API error 500: boom"


def test_ext_from_url():
    assert ext_from_url("https://x/y/a.mp4?sig=1") == ".mp4"
    assert ext_from_url("https://x/y/a.jpeg") == ".jpeg"
    assert ext_from_url("https://x/y/blob") == ".png"


def test_build_filename():
    assert build_filename("image", ".png", 1700) == "image_1700.png"
    assert build_filename("speech", "mp3", 42) == "speech_42.mp3"


def test_looks_remote():
    assert looks_remote("https://x/a.png")
    assert looks_remote("data:image/png;base64,AAA")
    assert not looks_remote("/home/u/a.png")


def test_to_image_field_url_passthrough():
    url = "https://x/a.png"
    assert to_image_field(url) == url


def test_to_image_field_missing_file_raises():
    with pytest.raises(SiliconFlowError, match="not found"):
        to_image_field("/no/such/file.png")


def test_to_image_field_local_file(tmp_path):
    f = tmp_path / "pic.png"
    f.write_bytes(b"\x89PNG")
    out = to_image_field(str(f))
    assert out.startswith("data:image/png;base64,")
    assert base64.b64decode(out.split(",", 1)[1]) == b"\x89PNG"
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `uv run --directory /Volumes/TBU4/Workspace/Aleph-mcp/siliconflow --extra dev pytest tests/test_client.py -v`
Expected: FAIL — `ModuleNotFoundError: No module named 'aleph_siliconflow_mcp.client'`

- [ ] **Step 3: Implement `client.py`**

`siliconflow/src/aleph_siliconflow_mcp/client.py`:
```python
"""Config, HTTP client, error handling, and local asset saving."""

from __future__ import annotations

import base64
import json as _json
import os
import time
from dataclasses import dataclass
from pathlib import Path
from urllib.parse import urlparse

import httpx

DEFAULT_API_BASE = "https://api.siliconflow.cn/v1"
REQUEST_TIMEOUT = 300.0


class SiliconFlowError(Exception):
    """User-facing error from the SiliconFlow API or local IO."""


@dataclass(frozen=True)
class Settings:
    api_key: str
    api_base: str
    image_dir: str | None
    audio_dir: str | None

    @classmethod
    def from_env(cls) -> "Settings":
        image_dir = os.getenv("SILICONFLOW_IMAGE_DIR") or None
        audio_dir = os.getenv("SILICONFLOW_AUDIO_DIR") or image_dir
        api_base = (os.getenv("SILICONFLOW_API_BASE") or DEFAULT_API_BASE).rstrip("/")
        return cls(
            api_key=os.getenv("SILICONFLOW_API_KEY", "").strip(),
            api_base=api_base,
            image_dir=image_dir,
            audio_dir=audio_dir,
        )


def extract_api_error(status_code: int, body: str) -> str:
    """Best-effort human-readable message from an error response body (pure)."""
    message = body
    try:
        parsed = _json.loads(body)
        if isinstance(parsed, dict):
            err = parsed.get("error")
            err_msg = err.get("message") if isinstance(err, dict) else err
            message = parsed.get("message") or err_msg or body
    except (ValueError, AttributeError):
        pass
    return f"SiliconFlow API error {status_code}: {message}"


def ext_from_url(url: str, default: str = ".png") -> str:
    """Derive a file extension from a URL path (pure)."""
    path = urlparse(url).path.lower()
    for ext in (".png", ".jpg", ".jpeg", ".mp4", ".webp"):
        if path.endswith(ext):
            return ext
    return default


def build_filename(prefix: str, ext: str, stamp: int) -> str:
    """Deterministic asset filename (pure)."""
    if not ext.startswith("."):
        ext = "." + ext
    return f"{prefix}_{stamp}{ext}"


def looks_remote(value: str) -> bool:
    """True if value is an http(s) URL or a data URI (pure)."""
    return value.startswith(("http://", "https://", "data:"))


def to_image_field(value: str) -> str:
    """Pass through a URL/data-URI; base64-encode a local file path."""
    if looks_remote(value):
        return value
    path = Path(value)
    if not path.is_file():
        raise SiliconFlowError(f"image file not found: {value}")
    data = base64.b64encode(path.read_bytes()).decode("ascii")
    suffix = path.suffix.lstrip(".") or "png"
    return f"data:image/{suffix};base64,{data}"


class SiliconFlowClient:
    def __init__(self, settings: Settings):
        if not settings.api_key:
            raise SiliconFlowError("SILICONFLOW_API_KEY is not set")
        self.settings = settings

    @property
    def _headers(self) -> dict[str, str]:
        return {"Authorization": f"Bearer {self.settings.api_key}"}

    def _url(self, path: str) -> str:
        return f"{self.settings.api_base}/{path.lstrip('/')}"

    async def request_json(self, method: str, path: str, *, json=None, params=None) -> dict:
        async with httpx.AsyncClient(timeout=REQUEST_TIMEOUT) as client:
            resp = await client.request(
                method, self._url(path), headers=self._headers, json=json, params=params
            )
        if resp.status_code >= 400:
            raise SiliconFlowError(extract_api_error(resp.status_code, resp.text))
        return resp.json()

    async def request_binary(self, method: str, path: str, *, json=None) -> bytes:
        async with httpx.AsyncClient(timeout=REQUEST_TIMEOUT) as client:
            resp = await client.request(
                method, self._url(path), headers=self._headers, json=json
            )
        if resp.status_code >= 400:
            raise SiliconFlowError(extract_api_error(resp.status_code, resp.text))
        return resp.content

    async def download(self, url: str, save_dir: str, prefix: str) -> str:
        """Download asset to save_dir; on any failure return the original URL."""
        try:
            target = Path(save_dir)
            target.mkdir(parents=True, exist_ok=True)
            async with httpx.AsyncClient(timeout=REQUEST_TIMEOUT) as client:
                resp = await client.get(url)
            resp.raise_for_status()
            name = build_filename(prefix, ext_from_url(url), int(time.time()))
            file_path = target / name
            file_path.write_bytes(resp.content)
            return str(file_path.resolve())
        except Exception:
            return url

    def save_binary(self, content: bytes, save_dir: str, ext: str, prefix: str) -> str:
        target = Path(save_dir)
        target.mkdir(parents=True, exist_ok=True)
        name = build_filename(prefix, ext, int(time.time()))
        file_path = target / name
        file_path.write_bytes(content)
        return str(file_path.resolve())


_client: SiliconFlowClient | None = None


def get_client() -> SiliconFlowClient:
    """Lazy singleton built from env (avoids import-time env reads)."""
    global _client
    if _client is None:
        _client = SiliconFlowClient(Settings.from_env())
    return _client


async def render_assets(
    client: SiliconFlowClient,
    kind: str,
    urls: list[str],
    save_dir: str | None,
    prefix: str,
    header: str,
) -> str:
    """Download each url (if save_dir set) and format an LLM-readable summary."""
    if not urls:
        return f"{header}: no {kind} returned by the API."
    lines = [header + ":"]
    for i, url in enumerate(urls, 1):
        if save_dir:
            local = await client.download(url, save_dir, prefix)
            lines.append(f"  {i}. {local}  (source: {url})")
        else:
            lines.append(
                f"  {i}. {url}  (remote URL, expires soon — set a save dir to keep it)"
            )
    return "\n".join(lines)
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `uv run --directory /Volumes/TBU4/Workspace/Aleph-mcp/siliconflow --extra dev pytest tests/test_client.py -v`
Expected: PASS (10 passed)

- [ ] **Step 5: Commit**

```bash
git -C /Volumes/TBU4/Workspace/Aleph-mcp add -A
git -C /Volumes/TBU4/Workspace/Aleph-mcp commit -q -m "feat: client config, http, error parsing, asset saving"
```

---

### Task 3: Images — generate_image / edit_image

**Files:**
- Create: `siliconflow/src/aleph_siliconflow_mcp/images.py`
- Test: `siliconflow/tests/test_images.py`

**Interfaces:**
- Consumes: `client.get_client`, `client.to_image_field`, `client.render_assets`; `ratios.image_size_for`.
- Produces:
  - `build_image_payload(*, prompt, model, image_size=None, negative_prompt=None, batch_size=1, seed=None, num_inference_steps=20, guidance_scale=None, cfg=None, images=None) -> dict`
  - `parse_image_response(data: dict) -> tuple[list[str], int | None]`
  - `async generate_image(...) -> str`, `async edit_image(...) -> str` (tools)

- [ ] **Step 1: Write the failing test**

`siliconflow/tests/test_images.py`:
```python
from aleph_siliconflow_mcp.images import build_image_payload, parse_image_response


def test_build_payload_minimal():
    p = build_image_payload(prompt="a cat", model="Kwai-Kolors/Kolors", image_size="1024x1024")
    assert p == {
        "model": "Kwai-Kolors/Kolors",
        "prompt": "a cat",
        "batch_size": 1,
        "num_inference_steps": 20,
        "image_size": "1024x1024",
    }


def test_build_payload_optionals_included_only_when_set():
    p = build_image_payload(
        prompt="x", model="m", image_size="1024x576",
        negative_prompt="blurry", seed=7, guidance_scale=5.0, cfg=4.0,
    )
    assert p["negative_prompt"] == "blurry"
    assert p["seed"] == 7
    assert p["guidance_scale"] == 5.0
    assert p["cfg"] == 4.0


def test_build_payload_edit_omits_image_size_and_carries_images():
    p = build_image_payload(
        prompt="add a hat", model="Qwen/Qwen-Image-Edit-2509",
        images={"image": "data:image/png;base64,AAA", "image2": None},
    )
    assert "image_size" not in p
    assert p["image"] == "data:image/png;base64,AAA"
    assert "image2" not in p  # None values dropped


def test_parse_image_response():
    data = {"images": [{"url": "https://x/a.png"}, {"url": "https://x/b.png"}], "seed": 99}
    urls, seed = parse_image_response(data)
    assert urls == ["https://x/a.png", "https://x/b.png"]
    assert seed == 99


def test_parse_image_response_empty():
    urls, seed = parse_image_response({})
    assert urls == []
    assert seed is None
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `uv run --directory /Volumes/TBU4/Workspace/Aleph-mcp/siliconflow --extra dev pytest tests/test_images.py -v`
Expected: FAIL — `ModuleNotFoundError: No module named 'aleph_siliconflow_mcp.images'`

- [ ] **Step 3: Implement `images.py`**

`siliconflow/src/aleph_siliconflow_mcp/images.py`:
```python
"""Image generation and editing tools (POST /v1/images/generations)."""

from __future__ import annotations

from .client import get_client, render_assets, to_image_field
from .ratios import image_size_for


def build_image_payload(
    *,
    prompt: str,
    model: str,
    image_size: str | None = None,
    negative_prompt: str | None = None,
    batch_size: int = 1,
    seed: int | None = None,
    num_inference_steps: int = 20,
    guidance_scale: float | None = None,
    cfg: float | None = None,
    images: dict | None = None,
) -> dict:
    payload: dict = {
        "model": model,
        "prompt": prompt,
        "batch_size": batch_size,
        "num_inference_steps": num_inference_steps,
    }
    if image_size:
        payload["image_size"] = image_size
    if negative_prompt:
        payload["negative_prompt"] = negative_prompt
    if seed is not None:
        payload["seed"] = seed
    if guidance_scale is not None:
        payload["guidance_scale"] = guidance_scale
    if cfg is not None:
        payload["cfg"] = cfg
    for key, value in (images or {}).items():
        if value:
            payload[key] = value
    return payload


def parse_image_response(data: dict) -> tuple[list[str], int | None]:
    urls = [img["url"] for img in data.get("images", []) if img.get("url")]
    return urls, data.get("seed")


async def generate_image(
    prompt: str,
    model: str = "Kwai-Kolors/Kolors",
    aspect_ratio: str = "1:1",
    negative_prompt: str | None = None,
    batch_size: int = 1,
    seed: int | None = None,
    num_inference_steps: int = 20,
    guidance_scale: float | None = None,
    cfg: float | None = None,
) -> str:
    """Generate image(s) from a text prompt via SiliconFlow. Returns local paths and URLs."""
    client = get_client()
    payload = build_image_payload(
        prompt=prompt,
        model=model,
        image_size=image_size_for(aspect_ratio),
        negative_prompt=negative_prompt,
        batch_size=batch_size,
        seed=seed,
        num_inference_steps=num_inference_steps,
        guidance_scale=guidance_scale,
        cfg=cfg,
    )
    data = await client.request_json("POST", "/images/generations", json=payload)
    urls, out_seed = parse_image_response(data)
    header = f"Generated {len(urls)} image(s) with {model} (seed={out_seed})"
    return await render_assets(client, "image", urls, client.settings.image_dir, "image", header)


async def edit_image(
    prompt: str,
    image: str,
    model: str = "Qwen/Qwen-Image-Edit-2509",
    image2: str | None = None,
    image3: str | None = None,
    negative_prompt: str | None = None,
    seed: int | None = None,
) -> str:
    """Edit / transform an image (local path or URL) with a text instruction."""
    client = get_client()
    images = {"image": to_image_field(image)}
    if image2:
        images["image2"] = to_image_field(image2)
    if image3:
        images["image3"] = to_image_field(image3)
    payload = build_image_payload(
        prompt=prompt, model=model, negative_prompt=negative_prompt, seed=seed, images=images
    )
    data = await client.request_json("POST", "/images/generations", json=payload)
    urls, out_seed = parse_image_response(data)
    header = f"Edited image with {model} (seed={out_seed})"
    return await render_assets(client, "image", urls, client.settings.image_dir, "edit", header)
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `uv run --directory /Volumes/TBU4/Workspace/Aleph-mcp/siliconflow --extra dev pytest tests/test_images.py -v`
Expected: PASS (5 passed)

- [ ] **Step 5: Commit**

```bash
git -C /Volumes/TBU4/Workspace/Aleph-mcp add -A
git -C /Volumes/TBU4/Workspace/Aleph-mcp commit -q -m "feat: generate_image and edit_image tools"
```

---

### Task 4: Videos — submit / status / generate (polling)

**Files:**
- Create: `siliconflow/src/aleph_siliconflow_mcp/videos.py`
- Test: `siliconflow/tests/test_videos.py`

**Interfaces:**
- Consumes: `client.get_client`, `client.to_image_field`, `client.render_assets`, `client.SiliconFlowError`; `ratios.video_size_for`.
- Produces:
  - `build_video_payload(*, prompt, model, image_size, image=None, negative_prompt=None, seed=None) -> dict`
  - `parse_submit_response(data: dict) -> str` (returns requestId; raises `SiliconFlowError` if absent)
  - `parse_status_response(data: dict) -> dict` (`{"status","reason","urls"}`)
  - `async submit_video_generation(...) -> str`, `async get_video_status(request_id) -> str`, `async generate_video(...) -> str`

- [ ] **Step 1: Write the failing test**

`siliconflow/tests/test_videos.py`:
```python
import pytest

from aleph_siliconflow_mcp.client import SiliconFlowError
from aleph_siliconflow_mcp.videos import (
    build_video_payload,
    parse_status_response,
    parse_submit_response,
)


def test_build_video_payload_t2v():
    p = build_video_payload(prompt="a wave", model="Wan-AI/Wan2.2-T2V-A14B", image_size="1280x720")
    assert p == {"model": "Wan-AI/Wan2.2-T2V-A14B", "prompt": "a wave", "image_size": "1280x720"}


def test_build_video_payload_i2v_with_options():
    p = build_video_payload(
        prompt="pan", model="Wan-AI/Wan2.2-I2V-A14B", image_size="960x960",
        image="https://x/a.png", negative_prompt="shaky", seed=3,
    )
    assert p["image"] == "https://x/a.png"
    assert p["negative_prompt"] == "shaky"
    assert p["seed"] == 3


def test_parse_submit_ok():
    assert parse_submit_response({"requestId": "req-1"}) == "req-1"


def test_parse_submit_missing_raises():
    with pytest.raises(SiliconFlowError):
        parse_submit_response({"oops": 1})


def test_parse_status_succeed():
    data = {"status": "Succeed", "results": {"videos": [{"url": "https://x/v.mp4"}]}}
    out = parse_status_response(data)
    assert out["status"] == "Succeed"
    assert out["urls"] == ["https://x/v.mp4"]


def test_parse_status_failed_carries_reason():
    out = parse_status_response({"status": "Failed", "reason": "nsfw"})
    assert out["status"] == "Failed"
    assert out["reason"] == "nsfw"
    assert out["urls"] == []


def test_parse_status_in_progress():
    out = parse_status_response({"status": "InProgress"})
    assert out["status"] == "InProgress"
    assert out["urls"] == []
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `uv run --directory /Volumes/TBU4/Workspace/Aleph-mcp/siliconflow --extra dev pytest tests/test_videos.py -v`
Expected: FAIL — `ModuleNotFoundError: No module named 'aleph_siliconflow_mcp.videos'`

- [ ] **Step 3: Implement `videos.py`**

`siliconflow/src/aleph_siliconflow_mcp/videos.py`:
```python
"""Video generation tools (POST /v1/video/submit + /v1/video/status)."""

from __future__ import annotations

import asyncio

from .client import SiliconFlowError, get_client, render_assets, to_image_field
from .ratios import video_size_for


def build_video_payload(
    *,
    prompt: str,
    model: str,
    image_size: str,
    image: str | None = None,
    negative_prompt: str | None = None,
    seed: int | None = None,
) -> dict:
    payload: dict = {"model": model, "prompt": prompt, "image_size": image_size}
    if image:
        payload["image"] = image
    if negative_prompt:
        payload["negative_prompt"] = negative_prompt
    if seed is not None:
        payload["seed"] = seed
    return payload


def parse_submit_response(data: dict) -> str:
    request_id = data.get("requestId")
    if not request_id:
        raise SiliconFlowError(f"unexpected video submit response: {data}")
    return request_id


def parse_status_response(data: dict) -> dict:
    results = data.get("results") or {}
    return {
        "status": data.get("status", "Unknown"),
        "reason": data.get("reason"),
        "urls": [v["url"] for v in results.get("videos", []) if v.get("url")],
    }


async def submit_video_generation(
    prompt: str,
    model: str = "Wan-AI/Wan2.2-T2V-A14B",
    aspect_ratio: str = "16:9",
    image: str | None = None,
    negative_prompt: str | None = None,
    seed: int | None = None,
) -> str:
    """Submit a video generation job; returns a requestId to poll with get_video_status."""
    client = get_client()
    payload = build_video_payload(
        prompt=prompt,
        model=model,
        image_size=video_size_for(aspect_ratio),
        image=to_image_field(image) if image else None,
        negative_prompt=negative_prompt,
        seed=seed,
    )
    data = await client.request_json("POST", "/video/submit", json=payload)
    request_id = parse_submit_response(data)
    return f"Video job submitted. requestId: {request_id}\nPoll with get_video_status."


async def get_video_status(request_id: str) -> str:
    """Check a video job's status; on success returns the saved path / URL."""
    client = get_client()
    data = await client.request_json("POST", "/video/status", json={"requestId": request_id})
    status = parse_status_response(data)
    if status["status"] == "Succeed":
        return await render_assets(
            client, "video", status["urls"], client.settings.image_dir, "video",
            f"Video ready (requestId {request_id})",
        )
    if status["status"] == "Failed":
        return f"Video generation failed: {status['reason'] or 'unknown reason'}"
    return f"Video status: {status['status']} (still processing). Poll again with requestId {request_id}."


async def generate_video(
    prompt: str,
    model: str = "Wan-AI/Wan2.2-T2V-A14B",
    aspect_ratio: str = "16:9",
    image: str | None = None,
    negative_prompt: str | None = None,
    seed: int | None = None,
    max_wait_seconds: int = 600,
    poll_interval_seconds: int = 5,
) -> str:
    """Generate a video and poll until done (with a timeout). Returns the saved path / URL."""
    client = get_client()
    payload = build_video_payload(
        prompt=prompt,
        model=model,
        image_size=video_size_for(aspect_ratio),
        image=to_image_field(image) if image else None,
        negative_prompt=negative_prompt,
        seed=seed,
    )
    submit = await client.request_json("POST", "/video/submit", json=payload)
    request_id = parse_submit_response(submit)
    waited = 0
    while waited < max_wait_seconds:
        await asyncio.sleep(poll_interval_seconds)
        waited += poll_interval_seconds
        data = await client.request_json("POST", "/video/status", json={"requestId": request_id})
        status = parse_status_response(data)
        if status["status"] == "Succeed":
            return await render_assets(
                client, "video", status["urls"], client.settings.image_dir, "video",
                f"Generated video with {model}",
            )
        if status["status"] == "Failed":
            return f"Video generation failed: {status['reason'] or 'unknown reason'}"
    return (
        f"Video still processing after {max_wait_seconds}s. "
        f"Poll later with get_video_status, requestId: {request_id}."
    )
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `uv run --directory /Volumes/TBU4/Workspace/Aleph-mcp/siliconflow --extra dev pytest tests/test_videos.py -v`
Expected: PASS (7 passed)

- [ ] **Step 5: Commit**

```bash
git -C /Volumes/TBU4/Workspace/Aleph-mcp add -A
git -C /Volumes/TBU4/Workspace/Aleph-mcp commit -q -m "feat: video submit/status/generate tools"
```

---

### Task 5: Audio — generate_speech (TTS)

**Files:**
- Create: `siliconflow/src/aleph_siliconflow_mcp/audio.py`
- Test: `siliconflow/tests/test_audio.py`

**Interfaces:**
- Consumes: `client.get_client`.
- Produces:
  - `build_speech_payload(*, input, model, voice=None, response_format="mp3", speed=1.0, gain=0.0) -> dict`
  - `ext_for_format(response_format: str) -> str`
  - `async generate_speech(...) -> str`

- [ ] **Step 1: Write the failing test**

`siliconflow/tests/test_audio.py`:
```python
from aleph_siliconflow_mcp.audio import build_speech_payload, ext_for_format


def test_build_speech_payload_defaults():
    p = build_speech_payload(input="hello", model="FunAudioLLM/CosyVoice2-0.5B")
    assert p == {
        "model": "FunAudioLLM/CosyVoice2-0.5B",
        "input": "hello",
        "response_format": "mp3",
        "speed": 1.0,
        "gain": 0.0,
        "stream": False,
    }


def test_build_speech_payload_with_voice():
    p = build_speech_payload(
        input="hi", model="m", voice="m:alex", response_format="wav", speed=1.5, gain=2.0
    )
    assert p["voice"] == "m:alex"
    assert p["response_format"] == "wav"
    assert p["speed"] == 1.5
    assert p["gain"] == 2.0


def test_ext_for_format():
    assert ext_for_format("wav") == "wav"
    assert ext_for_format("opus") == "opus"
    assert ext_for_format("unknown") == "mp3"
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `uv run --directory /Volumes/TBU4/Workspace/Aleph-mcp/siliconflow --extra dev pytest tests/test_audio.py -v`
Expected: FAIL — `ModuleNotFoundError: No module named 'aleph_siliconflow_mcp.audio'`

- [ ] **Step 3: Implement `audio.py`**

`siliconflow/src/aleph_siliconflow_mcp/audio.py`:
```python
"""Text-to-speech tool (POST /v1/audio/speech)."""

from __future__ import annotations

from .client import get_client

_AUDIO_EXTENSIONS = {"mp3", "opus", "wav", "pcm"}


def build_speech_payload(
    *,
    input: str,
    model: str,
    voice: str | None = None,
    response_format: str = "mp3",
    speed: float = 1.0,
    gain: float = 0.0,
) -> dict:
    payload: dict = {
        "model": model,
        "input": input,
        "response_format": response_format,
        "speed": speed,
        "gain": gain,
        "stream": False,
    }
    if voice:
        payload["voice"] = voice
    return payload


def ext_for_format(response_format: str) -> str:
    return response_format if response_format in _AUDIO_EXTENSIONS else "mp3"


async def generate_speech(
    input: str,
    model: str = "FunAudioLLM/CosyVoice2-0.5B",
    voice: str | None = None,
    response_format: str = "mp3",
    speed: float = 1.0,
    gain: float = 0.0,
) -> str:
    """Synthesize speech from text. voice format is 'model:voice_id'. Returns the saved path."""
    client = get_client()
    payload = build_speech_payload(
        input=input, model=model, voice=voice,
        response_format=response_format, speed=speed, gain=gain,
    )
    content = await client.request_binary("POST", "/audio/speech", json=payload)
    save_dir = client.settings.audio_dir
    if not save_dir:
        return (
            f"Generated {len(content)} bytes of {response_format} audio, but no save dir is set. "
            "Set SILICONFLOW_AUDIO_DIR (or SILICONFLOW_IMAGE_DIR) to save it."
        )
    path = client.save_binary(content, save_dir, ext_for_format(response_format), "speech")
    return f"Generated speech with {model}: {path}"
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `uv run --directory /Volumes/TBU4/Workspace/Aleph-mcp/siliconflow --extra dev pytest tests/test_audio.py -v`
Expected: PASS (3 passed)

- [ ] **Step 5: Commit**

```bash
git -C /Volumes/TBU4/Workspace/Aleph-mcp add -A
git -C /Volumes/TBU4/Workspace/Aleph-mcp commit -q -m "feat: generate_speech (TTS) tool"
```

---

### Task 6: User — get_user_info / list_models

**Files:**
- Create: `siliconflow/src/aleph_siliconflow_mcp/user.py`
- Test: `siliconflow/tests/test_user.py`

**Interfaces:**
- Consumes: `client.get_client`.
- Produces:
  - `parse_user_info(data: dict) -> dict` (`name,email,total_balance,charge_balance,gift_balance`)
  - `parse_model_list(data: dict) -> list[str]`
  - `async get_user_info() -> str`, `async list_models(type=None, sub_type=None) -> str`

- [ ] **Step 1: Write the failing test**

`siliconflow/tests/test_user.py`:
```python
from aleph_siliconflow_mcp.user import parse_model_list, parse_user_info


def test_parse_user_info_with_data_envelope():
    data = {"code": 20000, "data": {
        "name": "alice", "email": "a@x.com",
        "totalBalance": "88.88", "chargeBalance": "88.00", "balance": "0.88",
    }}
    info = parse_user_info(data)
    assert info == {
        "name": "alice", "email": "a@x.com",
        "total_balance": "88.88", "charge_balance": "88.00", "gift_balance": "0.88",
    }


def test_parse_user_info_flat_fallback():
    info = parse_user_info({"name": "bob", "totalBalance": "1.0"})
    assert info["name"] == "bob"
    assert info["total_balance"] == "1.0"


def test_parse_model_list():
    data = {"object": "list", "data": [{"id": "m1"}, {"id": "m2"}, {"nope": 1}]}
    assert parse_model_list(data) == ["m1", "m2"]
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `uv run --directory /Volumes/TBU4/Workspace/Aleph-mcp/siliconflow --extra dev pytest tests/test_user.py -v`
Expected: FAIL — `ModuleNotFoundError: No module named 'aleph_siliconflow_mcp.user'`

- [ ] **Step 3: Implement `user.py`**

`siliconflow/src/aleph_siliconflow_mcp/user.py`:
```python
"""Account + model-discovery tools (GET /v1/user/info, GET /v1/models)."""

from __future__ import annotations

from .client import get_client


def parse_user_info(data: dict) -> dict:
    d = data.get("data", data)
    return {
        "name": d.get("name"),
        "email": d.get("email"),
        "total_balance": d.get("totalBalance"),
        "charge_balance": d.get("chargeBalance"),
        "gift_balance": d.get("balance"),
    }


def parse_model_list(data: dict) -> list[str]:
    return [m["id"] for m in data.get("data", []) if m.get("id")]


async def get_user_info() -> str:
    """Show the SiliconFlow account profile and balances (total / charged / gift)."""
    client = get_client()
    data = await client.request_json("GET", "/user/info")
    info = parse_user_info(data)
    return (
        "SiliconFlow account:\n"
        f"  name: {info['name']}\n"
        f"  email: {info['email']}\n"
        f"  total balance: {info['total_balance']}\n"
        f"  charged: {info['charge_balance']}  gift: {info['gift_balance']}"
    )


async def list_models(type: str | None = None, sub_type: str | None = None) -> str:
    """List available models. type: text|image|audio|video; sub_type: e.g. text-to-image, text-to-video, text-to-speech."""
    client = get_client()
    params: dict = {}
    if type:
        params["type"] = type
    if sub_type:
        params["sub_type"] = sub_type
    data = await client.request_json("GET", "/models", params=params or None)
    models = parse_model_list(data)
    if not models:
        return "No models found."
    return "Available models:\n" + "\n".join(f"  - {m}" for m in models)
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `uv run --directory /Volumes/TBU4/Workspace/Aleph-mcp/siliconflow --extra dev pytest tests/test_user.py -v`
Expected: PASS (3 passed)

- [ ] **Step 5: Commit**

```bash
git -C /Volumes/TBU4/Workspace/Aleph-mcp add -A
git -C /Volumes/TBU4/Workspace/Aleph-mcp commit -q -m "feat: get_user_info and list_models tools"
```

---

### Task 7: Server registration + entry point

**Files:**
- Create: `siliconflow/src/aleph_siliconflow_mcp/server.py`
- Create: `siliconflow/src/aleph_siliconflow_mcp/main.py`
- Test: `siliconflow/tests/test_server.py`

**Interfaces:**
- Consumes: all eight tool functions from `images`, `videos`, `audio`, `user`.
- Produces: `server.mcp` (FastMCP instance named `aleph-siliconflow-mcp`), `server.main()`, `main.main` (re-export for the console script).

- [ ] **Step 1: Write the failing test**

`siliconflow/tests/test_server.py`:
```python
import asyncio


def test_server_imports_and_registers_eight_tools():
    from aleph_siliconflow_mcp import server

    assert server.mcp.name == "aleph-siliconflow-mcp"
    tools = asyncio.run(server.mcp.list_tools())
    names = {t.name for t in tools}
    assert names == {
        "generate_image",
        "edit_image",
        "generate_video",
        "submit_video_generation",
        "get_video_status",
        "generate_speech",
        "get_user_info",
        "list_models",
    }


def test_main_is_exported():
    from aleph_siliconflow_mcp.main import main

    assert callable(main)
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `uv run --directory /Volumes/TBU4/Workspace/Aleph-mcp/siliconflow --extra dev pytest tests/test_server.py -v`
Expected: FAIL — `ModuleNotFoundError: No module named 'aleph_siliconflow_mcp.server'`

- [ ] **Step 3: Implement `server.py` and `main.py`**

`siliconflow/src/aleph_siliconflow_mcp/server.py`:
```python
"""FastMCP server: registers all SiliconFlow media tools."""

from mcp.server.fastmcp import FastMCP

from . import audio, images, user, videos

mcp = FastMCP("aleph-siliconflow-mcp")

mcp.tool()(images.generate_image)
mcp.tool()(images.edit_image)
mcp.tool()(videos.generate_video)
mcp.tool()(videos.submit_video_generation)
mcp.tool()(videos.get_video_status)
mcp.tool()(audio.generate_speech)
mcp.tool()(user.get_user_info)
mcp.tool()(user.list_models)


def main() -> None:
    mcp.run()
```

`siliconflow/src/aleph_siliconflow_mcp/main.py`:
```python
"""Console-script entry point."""

from aleph_siliconflow_mcp.server import main

if __name__ == "__main__":
    main()
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `uv run --directory /Volumes/TBU4/Workspace/Aleph-mcp/siliconflow --extra dev pytest tests/test_server.py -v`
Expected: PASS (2 passed)

> If `mcp.list_tools()` is not the public API in the installed `mcp` version, the test will error on that line; in that case replace the body of `test_server_imports_and_registers_eight_tools` after the `name` assertion with `asyncio.run(server.mcp.list_tools())` adapted to the actual method (check `dir(server.mcp)`), keeping the eight-name set assertion.

- [ ] **Step 5: Run the full test suite**

Run: `uv run --directory /Volumes/TBU4/Workspace/Aleph-mcp/siliconflow --extra dev pytest -v`
Expected: PASS (all tests across the six test files green)

- [ ] **Step 6: Commit**

```bash
git -C /Volumes/TBU4/Workspace/Aleph-mcp add -A
git -C /Volumes/TBU4/Workspace/Aleph-mcp commit -q -m "feat: FastMCP server registration + entry point"
```

---

### Task 8: Docs + local uvx smoke test

**Files:**
- Create: `README.md` (repo root), `siliconflow/README.md`, finalize `siliconflow/.env.example`

**Interfaces:** none (docs + manual verification).

- [ ] **Step 1: Write `siliconflow/.env.example`**

```
SILICONFLOW_API_KEY=your_api_key_here
# Optional: override region endpoint (default https://api.siliconflow.cn/v1; overseas: .com)
SILICONFLOW_API_BASE=
# Optional: save generated images/videos locally (else only remote URLs are returned)
SILICONFLOW_IMAGE_DIR=
# Optional: save generated audio (defaults to SILICONFLOW_IMAGE_DIR)
SILICONFLOW_AUDIO_DIR=
```

- [ ] **Step 2: Write the repo-root `README.md` (charter + index + credits)**

Content must include:
- Title `# Aleph-mcp` and the **charter** (verbatim intent): *Aleph-mcp only fills official gaps — where a vendor ships no official MCP, Aleph builds one here; where an official MCP exists (e.g. Volcengine veImageX), Aleph's catalog points at the upstream source instead of duplicating it.*
- A **Servers** table: `siliconflow/` → "SiliconFlow media generation (image / video / TTS)".
- **Credits:** API knowledge referenced from the community project `stevefordev/siliconflow-mcp` (MIT); endpoints verified against official docs `https://api-docs.siliconflow.cn`. This is a clean rewrite.

- [ ] **Step 3: Write `siliconflow/README.md`**

Content must include: feature list (the 8 tools), the env-var table from `.env.example`, how to get a key (`https://cloud.siliconflow.cn/account/ak`), and a **Claude Code / MCP client config** block:
```json
{
  "mcpServers": {
    "siliconflow": {
      "command": "uvx",
      "args": ["--from", "git+https://github.com/rootazero/Aleph-mcp#subdirectory=siliconflow", "aleph-siliconflow-mcp"],
      "env": { "SILICONFLOW_API_KEY": "your_api_key_here", "SILICONFLOW_IMAGE_DIR": "/path/to/save" }
    }
  }
}
```
Note: media URLs expire quickly; set `SILICONFLOW_IMAGE_DIR` to keep assets.

- [ ] **Step 4: Local smoke — build & list tools via uvx**

Run (builds the package from the local checkout and runs the MCP handshake; requires a key in env for tools to execute, but listing works without calls):
```bash
SILICONFLOW_API_KEY=dummy uvx --from /Volumes/TBU4/Workspace/Aleph-mcp/siliconflow aleph-siliconflow-mcp --help 2>&1 | head -5 || echo "server starts (stdio MCP has no --help; Ctrl-C expected)"
```
Expected: the package builds without error (a stdio MCP server has no `--help`; the goal is to confirm `uvx` can build and launch the entry point). If it builds and blocks waiting for stdio, that is success — interrupt it.

- [ ] **Step 5: Commit**

```bash
git -C /Volumes/TBU4/Workspace/Aleph-mcp add -A
git -C /Volumes/TBU4/Workspace/Aleph-mcp commit -q -m "docs: repo charter, server README, env example"
```

---

### Task 9: Create GitHub repo and push

**Files:** none (repo publishing).

**Interfaces:** none. **OUTWARD-FACING ACTION — confirm with the user before running Step 2.**

- [ ] **Step 1: Verify clean tree and review log**

```bash
git -C /Volumes/TBU4/Workspace/Aleph-mcp status
git -C /Volumes/TBU4/Workspace/Aleph-mcp log --oneline
```
Expected: clean working tree; ~8 commits from Tasks 1–8.

- [ ] **Step 2: Create the public repo and push (after user confirmation)**

```bash
gh repo create rootazero/Aleph-mcp --public --source=/Volumes/TBU4/Workspace/Aleph-mcp --remote=origin --push \
  --description "Aleph official MCP servers — fills gaps where vendors ship no official MCP (first: SiliconFlow media generation)"
```
Expected: repo created at `https://github.com/rootazero/Aleph-mcp`; default branch pushed.

- [ ] **Step 3: Verify the uvx-from-git install path works end to end**

```bash
SILICONFLOW_API_KEY=dummy uvx --from "git+https://github.com/rootazero/Aleph-mcp#subdirectory=siliconflow" aleph-siliconflow-mcp 2>&1 | head -3 &
sleep 20 && kill %1 2>/dev/null; echo "if it built & launched (blocking on stdio), the catalog transport is valid"
```
Expected: `uvx` clones the repo, builds the subdir package, and launches the stdio server (blocks). This is the exact command the Aleph catalog preset will use.

---

### Task 10: Wire into Aleph's MCP preset catalog — the 5th built-in preset (main repo)

> **"Default-mounted" clarification (verified):** Aleph has NO runtime auto-mount of MCP servers. The four existing entries (context7, amap, minimax, volcengine-veimagex) are *built-in preset catalog* entries surfaced in Panel settings for one-click install via the `mcp.install_preset` RPC (the user supplies the API key). Adding `siliconflow` to `catalog.json` makes it the **5th built-in preset, on identical footing with veImageX and Amap** — which is exactly the user's "默认挂载第五个 MCP" requirement. No separate default-registration mechanism exists or is needed.

**Files (NOTE: edit the WORKTREE, not main — see footgun guard):**
- Modify: `/Volumes/TBU4/Workspace/Aleph/.claude/worktrees/siliconflow-mcp/src/mcp/presets/catalog.json` (append one preset → 5 total)
- Modify: `/Volumes/TBU4/Workspace/Aleph/.claude/worktrees/siliconflow-mcp/src/mcp/presets/mod.rs` (add `"siliconflow"` to the parse-test id list + update its comment, ~line 241-242)

> **🔴 Footgun guard:** This work runs in the git worktree `worktree-siliconflow-mcp` at `/Volumes/TBU4/Workspace/Aleph/.claude/worktrees/siliconflow-mcp`. Editing the main-repo absolute path (`/Volumes/TBU4/Workspace/Aleph/src/...`) would silently bypass the worktree and land on `main`, breaking branch isolation. Always use the worktree path above.

**Interfaces:**
- Consumes: the published repo URL from Task 9.
- Produces: a one-click installable `siliconflow` preset surfaced in the Panel as the 5th official built-in MCP.

- [ ] **Step 1: Add the preset to `catalog.json`**

Insert a comma after the closing `}` of the `volcengine-veimagex` object (currently the last array element), then add this object before the final `]`:
```json
,
  {
    "id": "siliconflow",
    "name": "硅基流动 SiliconFlow",
    "category": "model-provider",
    "description": "文生图 / 图生视频 / 语音合成（Aleph 自建官方 MCP）。",
    "vendor": "硅基流动 SiliconFlow",
    "official": true,
    "reachability": "cn-native",
    "transports": [
      { "kind": "stdio", "command": "uvx", "args": ["--from", "git+https://github.com/rootazero/Aleph-mcp#subdirectory=siliconflow", "aleph-siliconflow-mcp"], "requires_runtime": "python" }
    ],
    "required_env": [
      { "key": "SILICONFLOW_API_KEY", "label": "SiliconFlow API Key", "description": "平台 API Key", "secret": true, "required": true, "how_to_get_url": "https://cloud.siliconflow.cn/account/ak" },
      { "key": "SILICONFLOW_API_BASE", "label": "API Base", "description": "区域接入点（默认 .cn，海外可改 .com）", "secret": false, "required": false, "default": "https://api.siliconflow.cn/v1" },
      { "key": "SILICONFLOW_IMAGE_DIR", "label": "图片/视频保存目录", "description": "本地落盘目录（留空只返回远程 URL）", "secret": false, "required": false },
      { "key": "SILICONFLOW_AUDIO_DIR", "label": "音频保存目录", "description": "默认回退到图片目录", "secret": false, "required": false }
    ],
    "tags": ["image", "video", "tts", "model-provider"]
  }
```

- [ ] **Step 2: Add the id assertion in `mod.rs`**

Find (≈ line 241-242):
```rust
        // 首批 4 个 id 必须都在
        for id in ["context7", "amap", "minimax", "volcengine-veimagex"] {
```
Replace with:
```rust
        // 内置 5 个官方预设 id 必须都在
        for id in ["context7", "amap", "minimax", "volcengine-veimagex", "siliconflow"] {
```

- [ ] **Step 3: Run the catalog parse test (single, scoped — respects cargo frugality)**

Run: `cargo test -p alephcore --lib presets`
Expected: PASS, including `bundled_catalog_parses_and_has_first_batch` (now asserts the `siliconflow` preset parses and is found).

- [ ] **Step 4: Commit (main repo)**

```bash
WT=/Volumes/TBU4/Workspace/Aleph/.claude/worktrees/siliconflow-mcp
git -C "$WT" add src/mcp/presets/catalog.json src/mcp/presets/mod.rs
git -C "$WT" commit -m "mcp: add SiliconFlow preset (Aleph-mcp self-built)"
```

---

## Self-Review

**1. Spec coverage:**
- §1 boundary/charter → Task 8 (README charter), Task 10 (preset). ✓
- §2 decisions (rewrite/monorepo/uvx-from-git/Python/.cn) → Tasks 1–9. ✓
- §3 endpoint corrections → Tasks 3 (images same endpoint + cfg/image2/3), 4 (Wan2.2 + image_size whitelist + `Succeed` + `results.videos[].url`), 5 (TTS binary + voice), 6 (models sub_type, user balances). ✓
- §4 tool specs (8 tools) → Tasks 3–7. ✓
- §5 env config → Tasks 2 (Settings), 8 (.env.example), 10 (preset env). ✓
- §6 error handling → Task 2 (extract_api_error, missing-key, download fallback), 4 (poll timeout), 5 (no-dir graceful). ✓
- §7 repo structure → Tasks 1–8. ✓
- §8 Aleph integration → Task 10. ✓
- §9 testing → pure-function pytest in Tasks 1–6; server smoke Task 7; Rust test Task 10. ✓
- §10 delivery sequence → task order matches. ✓

**2. Placeholder scan:** No TBD/TODO; every code step shows complete code; the one conditional note (Task 7 Step 4) gives an explicit fallback procedure, not a placeholder. ✓

**3. Type consistency:** `render_assets(client, kind, urls, save_dir, prefix, header)` — defined in Task 2, called identically in Tasks 3 & 4. `build_image_payload`/`parse_image_response`/`build_video_payload`/`parse_submit_response`/`parse_status_response`/`build_speech_payload`/`ext_for_format`/`parse_user_info`/`parse_model_list` — signatures in Interfaces blocks match their implementations and tests. `get_client()`/`Settings`/`to_image_field`/`SiliconFlowError` consumed consistently. ✓

---

## Execution Handoff

Plan complete. Two execution options:

1. **Subagent-Driven (recommended)** — a fresh subagent per task, two-stage review between tasks, fast iteration.
2. **Inline Execution** — execute tasks in this session with checkpoints.

Note: Task 9 (GitHub repo creation) is an outward-facing action requiring explicit user confirmation regardless of mode.
