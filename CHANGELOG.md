# Changelog

All notable changes to the Aleph project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.11] - 2026-03-23

### Added
- webchat: add i18n infrastructure with leptos_i18n v0.6
- panel: i18n all pages — dashboard, chat, settings, agents, cron, memory, logs
- panel: i18n settings pages — plugins, skills, clawhub, policies, acp, providers
- panel: wire language switching in general settings

### Changed
- core: dead code cleanup — remove unused modules (question, spec_driven, suggestion)
- core: plugin discovery and manifest parsing improvements
- core: prompt builder and skill instruction updates

### Fixed
- build: fix install scripts — proper upgrade flow and service management
- panel: fix i18n reactivity, remove dead code

## [0.2.10] - 2026-03-23

### Added
- core: gemini provider improvements
- core: generation builder enhancements
- core: telegram interface updates

### Changed
- core: rerank providers (jina, pinecone, siliconflow, vllm, voyage) improvements
- core: memory embedding provider updates
- webchat: settings UI refinements

## [0.2.9] - 2026-03-22

### Added
- core: codex provider improvements
- core: agent loop updates

### Changed
- webchat: panel UI refresh

## [0.2.8] - 2026-03-22

### Added
- build: unified version source — VERSION file drives all version strings
- build: `just release x.x.x` automated release recipe with changelog generation
- desktop-macos: implement AutomationCapability (osascript + Shortcuts CLI)
- desktop-macos: implement SystemCapability (apps, notifications, clipboard, sysinfo)
- desktop-macos: implement PimCapability via SwiftBridge
- apps: implement real macOS API calls in Swift CLI bridge (Notes, Calendar, Reminders, Contacts)
- desktop: add NativeScreen shared ScreenCapability implementation

### Changed
- core: rewire DesktopTool to dispatch via DesktopPlatform.screen()
- core: rewire PimTool to dispatch via DesktopPlatform.pim()
- core: remove legacy NativeDesktop, use DesktopPlatform for screen control
- phase4: remove Tauri, archive old apps, move Swift bridge to crates/desktop-macos/bridge
- phase4: clean all Tauri references from codebase
- build: rename workflows, fix --bin aleph→aleph-server, add platform release workflows
- build: update justfile and CI workflows for post-Tauri architecture

### Fixed
- fix: replace env!("HOME") with dirs::home_dir() for Windows compatibility
- build: update install scripts for aleph-server binary name

## [0.2.7] - 2026-03-22

### Added
- core: multi-agent system improvements
- webchat: UI updates

## [0.2.6] - 2026-03-21

### Added
- desktop: add capability trait hierarchy (Screen, PIM, System, Automation)
- desktop: add per-platform crate skeletons (macOS, Linux, Windows)
- desktop: add SwiftBridge utility for macOS native API calls
- core: add SystemTool and AutomationTool builtin tools
- apps: add Swift CLI bridge skeleton for macOS native APIs

### Changed
- core: wire up DesktopPlatform and register system/automation tools
- desktop: Phase 1 architecture scaffold complete
