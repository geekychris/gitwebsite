#!/usr/bin/env bash
# Install gitwebsite's Claude Code skills by symlinking skills/* into ~/.claude/skills/,
# then print how to register the MCP server.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEST="${CLAUDE_SKILLS_DIR:-$HOME/.claude/skills}"
mkdir -p "$DEST"
for src in "$ROOT"/skills/*/; do
  name="$(basename "$src")"
  target="$DEST/$name"
  if [ -L "$target" ]; then
    rm "$target"
  elif [ -e "$target" ]; then
    echo "skip $name: $target exists and is not a symlink (remove it to let gitwebsite manage it)" >&2
    continue
  fi
  ln -s "${src%/}" "$target"
  echo "linked $name -> ${src%/}"
done

GW="$ROOT/target/release/gw"
# The show-with-tools skill's tool index comes from the current site's tool pages.
if [ -x "$GW" ] && "$GW" tools index --out "$ROOT/skills/show-with-tools/tools-index.md" >/dev/null 2>&1; then
  echo "regenerated show-with-tools/tools-index.md from the current site"
fi
echo
echo "Register the MCP server (once; build first with: cargo build --release -p gw):"
echo "  claude mcp add --scope user gitwebsite -- $GW mcp"
