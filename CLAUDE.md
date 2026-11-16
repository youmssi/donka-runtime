# Donka Runtime — Claude Code instructions

@AGENTS.md

## Claude-specific

- Start every task by reading `AGENTS.md` and the story. Check dependencies are merged first.
- `rustfmt` defaults are the style authority. Typed errors in library code; no `unwrap()` outside
  tests and fail-fast startup code.
- Commit with the repository owner's identity; never add a tool attribution line, a "generated
  with" footer, or a co-author trailer. If a tool appends a footer to a PR, edit it out.
- When a story has an `[INTERACTIVE STEP]`, stop and present the options.
