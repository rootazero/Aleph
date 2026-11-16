# NSH Native UI / R2 Decision Proposal / NSH 原生 UI 与 R2 决策提案

**Status / 状态:** pending human/architecture approval; does not amend `AGENTS.md`
**Date / 日期:** 2026-11-15
**Track / 轨道:** NSH 0.5, separate from Capability Phase8

## Problem / 问题

Current R2 requires complex business UI in Leptos/WASM and keeps the bridge at OS/API I/O. The NSH replacement provides a native shell and a shared engine/JS/core model. A native shell can host the existing Panel contract, but a full business-UI port would change the current UI authority. The decision must be explicit before implementation.

## Proposed decision / 提议决策

For the first NSH slice, keep R2 intact: NSH owns native windows, platform lifecycle, transport and OS/API I/O; the existing Panel/Leptos/WASM business UI and its Server-derived state remain the business UI authority. NSH may redesign host composition for performance, but it must preserve the Panel's observable routes, state transitions and error semantics.

A later native business-UI port may be proposed as a separate ADR only after the parity matrix, performance evidence and capability boundary are complete. It must name the new UI authority, preserve Rust Core ownership, define the bridge contract and explain why the change improves performance or lifecycle isolation.

## Approval choices / 批准选项

1. **Approve the proposed clarification:** first slice keeps R2 and uses NSH as the native shell/host seam.
2. **Approve a native business-UI transition:** requires a new ADR with an explicit replacement authority and contract before implementation.
3. **Defer the NSH slice:** no dependency or production change until the architecture decision is recorded.

Until one choice is recorded, NSH implementation is blocked. This proposal is not an architectural amendment by itself.
