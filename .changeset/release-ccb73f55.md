---
"agent-browser": patch
---

Fixed Windows compatibility issues including proper handling of extended-length path prefixes from canonicalize(), prevention of MSYS/Git Bash path translation that could mangle arguments, and improved daemon startup reliability. Also added ARM64 Windows support in postinstall shims and expanded CI testing with a full daemon lifecycle test on Windows.
