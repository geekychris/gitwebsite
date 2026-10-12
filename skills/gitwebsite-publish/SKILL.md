---
name: gitwebsite-publish
description: Build, preview, check, and publish the user's website (a gitwebsite site, usually on GitHub Pages). Use when the user asks to publish the site, push a post live, release a draft, preview the site locally, rebuild it, check it for broken references, or see what a publish would change.
---

# gitwebsite-publish

The site is a static site built by **gitwebsite** (`gw`) from AsciiDoc and published to the
GitHub Pages repository named in its `site.yaml` (`publish.remote`).

- Site: `gw site` shows which one and where (`--site <dir>`, `$GW_SITE`, `site.yaml` here or above,
  or the current site in `gw config show`)
- Binary: `gw` — if it is not on PATH, it is `target/release/gw` in the gitwebsite checkout
  (build with `cargo build --release -p gw` there)
- MCP: the `gitwebsite` server exposes the same operations (tool names in brackets below)

**Publishing is outward-facing.** It updates the live public website. Never publish without
showing the user the plan and getting an explicit yes in this conversation.

## Workflow

1. **Check** — `gw check` [`check_site`]. Fix unknown areas, missing project slugs, unknown
   `media:` keys, missing `:date:` on posts, apps without `:url:`.
2. **Preview** — `gw serve --open` serves the management UI at http://127.0.0.1:8790/app and
   the site preview; or `gw build --drafts` [`build_site` with drafts=true] then open
   `site/dist/index.html` via a local server (`python3 -m http.server -d site/dist`).
   Drafts appear only in preview builds.
3. **Release** the items that should go live — `gw release <id>` [`set_status` published].
   `gw list --drafts` [`list_content` drafts_only] shows what is still draft.
4. **Plan** — `gw publish --dry-run` [`publish_plan`]. Show the user the added / updated /
   removed files. gitwebsite only touches files it built before (its manifest); paths under
   `publish.preserve` in `site.yaml` (books, résumé) are never touched.
5. **Confirm**, then publish — `gw publish -y -m "message"` [`publish_site` with confirm=true].
   `--no-push` commits to the Pages checkout without pushing.
6. Report the live URL (the site's `base_url` plus the page path); GitHub Pages takes a minute or two.
   Mark any linked ideas published: `gw idea set <n> published` [`update_idea`].

## Notes

- The Pages checkout lives at `publish.pages_repo` (set in site.yaml; `gw init` puts it under the gitwebsite config folder);
  `gw` clones it if missing and refuses to publish over uncommitted changes there.
- Unlisted items (`:visibility: unlisted`) are built with `noindex` and kept out of listings,
  feeds, and the sitemap — they are still reachable by URL.
