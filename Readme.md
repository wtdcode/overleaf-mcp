# overleaf-mcp

An MCP server for [Overleaf](https://www.overleaf.com), written in Rust. Works against both self-hosted Community Edition instances and www.overleaf.com, using the same realtime OT protocol as the web editor — edits are true live collaborative edits, safe to run while humans have the project open.

## Features

- **Projects & files**: list projects, list the file tree, `stat_file` (line count / size / version without reading content).
- **Read**: line-numbered reads with `offset`/`limit` line ranges.
- **Edit**: `edit_file` (exact string replacement, Claude-style unique-match semantics) and `write_file` (full overwrite, creates missing folders/docs). Both are applied as realtime OT operations: concurrent edits from other collaborators are preserved, interleaved edits are detected and resynced automatically. Read-before-edit is enforced.
- **Search**: regex search across all docs or a subtree, returning `path:line:` matches — also the intended way to get a document outline (search for `\\(section|subsection|...)`).
- **File management**: create folders, rename, move, delete (recursive), upload local files (binary assets or text docs), download docs/files to local paths.
- **Compile**: trigger a compile, get the status plus errors/warnings parsed from the LaTeX log; page through the full `output.log` with `read_log`; save `output.pdf` or any artifact with `download_output`.
- **History**: `get_history` (versions, authors, changed files, structural ops, labels, paging) and `label_version` to tag a project history version.
- **Project sandbox**: `--project` pins the server to one project — every tool call naming another project is denied, `list_projects` shows only the pinned one, and the MCP server instructions tell the model the project is preset (the `project` parameter can then be omitted).
- **Transports**: stdio (default) or Streamable HTTP (`--listen`).

## Install

Prebuilt binaries for Linux (x86_64, musl static), Windows (x86_64), and macOS (arm64 & x86_64) are available on the [GitHub Releases](https://github.com/wtdcode/overleaf-mcp/releases) page. Or build from source:

```bash
cargo build --release
```

## Configuration

Via flags or environment variables (a `.env` in the working directory is loaded automatically):

| Variable | Meaning |
|---|---|
| `OVERLEAF_ENDPOINT` | Base URL, e.g. `https://overleaf.example.com` |
| `OVERLEAF_ACCOUNT` / `OVERLEAF_PASSWORD` | Password login (Community Edition) |
| `OVERLEAF_COOKIE` | Browser session cookies, e.g. `overleaf_session2=...` — required for www.overleaf.com, whose password login is CAPTCHA-gated. Copy it from DevTools → Cookies while logged in |
| `OVERLEAF_PROJECT` | Optional: pin the server to this project (name or id) |

Credentials are either account+password or a session cookie; when both are present the cookie wins. Timeouts, edit retries, and search caps are tunable via flags (`--help`).

## Use with Claude Code

Over stdio (Claude Code spawns the server):

```bash
claude mcp add overleaf \
  --env OVERLEAF_ENDPOINT=https://overleaf.example.com \
  --env OVERLEAF_ACCOUNT=you@example.com \
  --env OVERLEAF_PASSWORD=... \
  --env OVERLEAF_PROJECT="My Paper" \
  -- /path/to/overleaf-mcp
```

Or over Streamable HTTP (run the server yourself, e.g. shared by several sessions):

```bash
overleaf-mcp --listen 127.0.0.1:3000   # reads .env from the working directory
claude mcp add --transport http overleaf http://127.0.0.1:3000
```

## Notes & limitations

- Characters outside the Unicode BMP (e.g. emoji) are rejected on write: Overleaf's document pipeline cannot store them and would corrupt them to U+FFFD.
- Docs migrated to Overleaf's newer `history-ot` format can be read and compiled but not yet edited; the server reports this explicitly.
- Overleaf rate-limits back-to-back compiles (`too-recently-compiled`); the previous compile's log and artifacts stay available.
- www.overleaf.com session cookies expire eventually and must be refreshed from a browser; headless re-login is impossible due to the CAPTCHA.
- All state (doc shadows, read tracking, compile results) is in memory; restarting the server just starts clean.

## License

MIT
