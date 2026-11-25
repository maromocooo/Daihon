<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/daihon-logo-dark.svg">
    <source media="(prefers-color-scheme: light)" srcset="assets/daihon-logo-light.svg">
    <img alt="Daihon" src="assets/daihon-logo-light.svg" width="320">
  </picture>
</p>

<p align="center">
  <strong>English</strong> | <a href="README.ja.md">日本語</a>
</p>

# Daihon — Scenario-Driven Development Harness for Coding Agents

**Daihon** (台本, *"script"*) keeps autonomous coding agents aligned with the end-to-end user scenarios you actually want to dogfood—through **natural-language scenario registration, independent watcher verification, a minimal TUI, drift detection, smartphone notifications (`ntfy`), and CLI agent integrations (`agy` / `codex` / `claude`)**.

> **You never need to hand-write YAML or Markdown.**  
> Just describe what you want in natural language via `daihon register "..."`. Daihon converts it into a human-readable Markdown + YAML front matter scenario, checks for overlaps with existing scenarios, and suggests a priority (`P0`–`P3`).

## Why Daihon?

When you delegate implementation to coding agents, they frequently drift toward small, isolated parts while postponing the core end-to-end workflow you want to dogfood first—and you often notice too late.

Daihon closes the loop:

1. **Always-On alignment**: Injects your `P0` scenarios and current progress into coding agent sessions (`AGENTS.md` / `CLAUDE.md` rules + `SessionStart` hooks).
2. **Independent watcher**: A **separate agent** (e.g., `codex` watching `claude`/`agy`) automatically tests each step in the background (`auto` checks via commands/SSH, while keeping `manual` GUI checks separate) and records exactly which step and check failed.
3. **Drift detection & automatic handoff**: Flags `[DRIFT]` when code changes keep happening without advancing the stopped step, and keeps `.daihon/NEXT_TASK.md` up to date so agents self-correct before you have to intervene.
4. **Signal-only phone notifications**: Sends a smartphone notification via [ntfy](https://ntfy.sh/) **only** when a scenario passes all steps—never spamming you on individual parts or partial progress.

## Install

Requires Rust / Cargo (`rustc 1.85+`, edition 2024):

```bash
git clone git@github.com:maromocooo/Daihon.git
cd Daihon
cargo install --path .
```

## Quick Start (New or Existing Repository)

```bash
cd /path/to/your-project
daihon init                           # Creates .daihon.toml and scenarios/
echo '.daihon/' >> .gitignore
daihon init-integration agy           # Or: codex / claude (merges Skill, rules & hooks safely)

# 1. Register your core dogfooding scenario in natural language
daihon register "Launch the terminal workflow and seamlessly continue development across hosts"

# 2. Verify with an independent watcher agent
daihon watch

# 3. Inspect priorities, step progress, and drift warnings in the minimal TUI
daihon tui

# 4. File a missing-feature task from the stopped step & run the approval-gated fix loop
daihon fix --plan-only                # Generate task file only
daihon fix                            # Approve -> developer implements -> watcher re-verifies
```

### Smartphone Notifications (`ntfy`)

Subscribe to a private topic in the iOS/Android [ntfy](https://ntfy.sh/) app and export it:

```bash
export DAIHON_NTFY_TOPIC="your-private-topic"
daihon notify test
```

## Command Reference

| Command | Description |
| :--- | :--- |
| `daihon init` | Initialize `.daihon.toml` and `scenarios/` in a repository |
| `daihon validate [path]` | Validate scenario Markdown/front matter format and status consistency |
| `daihon tui [--dir ...]` | Open the minimal `ratatui` TUI (list, priorities, step checks, `[DRIFT]` alerts) |
| `daihon status [--json]` | Print non-interactive scenario status summary and stopped steps |
| `daihon context` | Output concise current-state / drift brief for coding agent sessions |
| `daihon register "<text>"` | Turn natural language into a structured scenario with overlap & priority proposals |
| `daihon watch [--if-changed \| --background \| --interval 15m \| --stop \| --daemon-status]` | Run independent watcher verification or manage the background periodic daemon |
| `daihon check <id> <check-id> --pass` | Mark `[manual]` (or any) check items as passed/failed/reset |
| `daihon hook install` | Install a non-blocking git `post-commit` hook for background watching |
| `daihon fix [--plan-only \| --auto-approve]` | Generate a missing-feature task from the stopped step and run the dev → re-watch loop |
| `daihon notify test [--dry-run]` | Send a test notification via `ntfy` (or record locally with `--dry-run`) |
| `daihon init-integration <agy\|codex\|claude> [--no-rules \| --no-hooks]` | Safely install/merge Skills, Always-On rules, and lifecycle hooks |

## Documentation

- **Japanese README**: [README.ja.md](README.ja.md)
- **Getting Started & Daily Workflow (JA)**: [docs/getting-started.md](docs/getting-started.md)
- **Scenario File Format Specification (JA)**: [docs/scenario-format.md](docs/scenario-format.md)

## Development & Tests

```bash
cargo test && cargo clippy --all-targets -- -D warnings
```
