---
"agent-browser": patch
---

Fixed "Daemon not found" error when running through AI agents (e.g., Claude Code) by resolving symlinks in the executable path. Previously, npm global bin symlinks weren't being resolved correctly, causing intermittent daemon discovery failures.
