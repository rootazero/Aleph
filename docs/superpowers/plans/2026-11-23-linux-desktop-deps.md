# Linux NSH Desktop Dependencies Implementation Plan / Linux NSH 桌面依赖实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give NSH desktop candidate users a pre-launch check and an opt-in official package-manager installation path. / 提供启动前检测及显式安装入口。

**Architecture:** `scripts/install-desktop-deps.sh` owns the distro routing and loader-cache probe. Python integration tests invoke the real script with hermetic command fixtures; the current Arch desktop validates the real cache path.

**Tech Stack:** Bash, glibc `ldconfig`, Python `unittest`, Arch/Debian/Ubuntu/Fedora package managers.

**Spec:** [Linux NSH desktop dependency bootstrap](../specs/2026-11-23-linux-desktop-deps-design.md).

## Global Constraints / 全局约束

Check only by default. Exact IDs `arch`, `debian`, `ubuntu`, `fedora`; no package-manager auto-confirmation, system upgrades, extra repositories, signature-policy changes, server installer changes or default desktop changes.

## Review Focus / 审查重点

A wrong-architecture library, confusing GTK3/WebKitGTK 4.1 with GTK4/WebKitGTK 6, no loader cache, missing sudo/manager, package-manager failure, and successful package install with libraries still missing must not count as success.

### Task 1: Implement and verify the standalone entry / 实现并验证独立入口

- [x] Create `qa/test_install_desktop_deps.py`. Exercise already installed, missing `--check`, explicit installs on all mapped IDs, unsupported ID, wrong architecture or old libraries, missing/failed probe, manager failure, missing manager or privilege and failed post-install check. Run `python3 -m unittest discover -s qa -p test_install_desktop_deps.py` and confirm RED.
- [x] Create `scripts/install-desktop-deps.sh [--check|--install|--help]`, testing real loader-cache output and calling the mapped manager without auto-confirmation. Run the same tests for GREEN, `bash -n scripts/install-desktop-deps.sh`, then run `bash scripts/install-desktop-deps.sh --check` against this machine's actual loader cache.
- [x] Document opt-in, mapped packages, failure behavior and the NSH-only scope in `docs/reference/LINUX_DESKTOP.md`. / 双语说明。
- [x] Check the diff and commit locally. / 检查差异、仅本地提交。
