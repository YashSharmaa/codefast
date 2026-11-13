# codefast

This workspace has codefast, a prebuilt structural index of the repository. Its tools come from
the `codefast` MCP server (`search`, `show`, `refs`, `path`, `grep`). When `codefast` is on PATH
the same queries run from the shell (`codefast search NAME`, `codefast callers NAME`, ...).

Use it before text search plus reading files when the question is about a symbol:

- where something is defined, or the body of one function or type: `search`, `show`
- who calls or uses something, including through callback tables: `refs`
- how execution gets from one function to another: `path`
- which function prints a message, with the enclosing function of every match: `grep`
- where to start for a bug report or a stack trace: `search` with the pasted text

Answers list `path:line` locations that you can open directly, and say so when they are cut
short. Plain text search is still right for exact strings in configuration or documentation.
