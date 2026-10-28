---
name: gitwebsite-write
description: Write or update content for the user's website with gitwebsite — articles, projects, project updates, Lab apps, code playgrounds, browser tools (/tools/), and pages — in AsciiDoc, placed in the site's information architecture. Use when the user asks to write a post, blog about something, write up a project or a build, add a project update, add an app or playground to the Lab, or edit existing site content. For reworking an existing post with an interview and the code behind it, use enhance-post.
---

# gitwebsite-write

Content lives in the site's `content/` folder (`gw site` shows where).

Whose site and where: run `gw site` (or the MCP `site_info` tool) first — it gives the site's
title, author, address (`base_url`), folder, and publish target. Refer to the author by name
from there; never assume one. `gw` finds the site itself (`--site <dir>`, `$GW_SITE`, the folder
you are in, or the current site from `gw config show`).
Use the `gw` CLI or the `gitwebsite` MCP tools (in brackets). New content is always a **draft**;
the user reviews before anything is released or published (see `gitwebsite-publish`).

## 1. Decide kind and place

| Kind | Use for | Lives at | URL |
|---|---|---|---|
| `article` | a dated write-up of something made or learned | `content/articles/` | `/articles/slug/` |
| `project` | long-running work, software or physical | `content/projects/slug/index.adoc` | `/projects/slug/` |
| `update` | a dated progress post on a project (`--project slug`) | `content/projects/slug/updates/` | `/projects/slug/u/` |
| `app` | a publicly running app, shown in the Lab (`--url`) | `content/lab/apps/` | `/lab/apps/slug/` |
| `playground` | runnable code to edit in the browser | `content/lab/playground/` | `/lab/playground/slug/` |
| `page` | standalone pages (About) | `content/pages/` | `/slug/` |

Areas: run `gw areas` [`list_areas`]. An item can sit in **several** areas (e.g. a jig
printed for the bandsaw: `making/woodworking, making/3d-printing`). Use existing areas; if
something genuinely needs a new one, propose an edit to `site.yaml` rather than inventing it.

## 2. Gather material

From a repo: README, docs, `git log --oneline`, releases, screenshots. From a physical build:
the user's notes and photos (see `gitwebsite-media` for images). Check the registry and ideas
first: `gw registry list`, `gw idea list` [`list_registry`, `list_ideas`].

## 3. Create

```bash
gw new article "Building a crosscut sled" --area woodworking --tag jig --summary "One line."
gw new update "Phase 2: the enclosure" --project weather-station --area electronics
gw new app "boingtrace" --url https://... --area amiga
gw new article "Shop photos" --bundle     # directory form, so images sit beside index.adoc
```

[`create_content` with kind, title, areas, tags, summary, project, repo, url, body, bundle]
Then edit the file (or `update_content_source` with the full text from `get_content`).

## 4. AsciiDoc conventions

Header (one attribute per line, then a blank line):
`:type:` · `:date: YYYY-MM-DD` · `:areas:` · `:tags:` · `:project:` · `:summary:` (one line, used on
cards and feeds) · `:hero: media:key` · `:status: draft` · `:repo:` · `:url:` / `:embed: true` (apps)
· `:source: <registry id>` (provenance) · `:featured: true` · `:pinned: 1` (top of the home page;
the number orders pins).

- Images: `image::media:key[Alt text]` (library media), `image::photo.jpg[Alt]` (file beside a
  bundle's `index.adoc`), `image::https://…[Alt]`. Video: `video::media:key[]`.
- Runnable code: `[source.playground,javascript]` (or `python`, `html`) on a `----` block — readers
  can edit and run it.
- Normal code: `[source,rust]`. Sections with `==`. Cross-link with `link:/projects/x/[text]`.

## 5. Voice

First person, plain and specific: what it is, why it was made, how, what went wrong, what was
learned, what is next. Numbers and concrete details over adjectives. Credit sources and tools.
Never invent facts about the user's work — ask, or leave a clearly marked `// TODO`.

## Reworking an existing post

To substantially improve a post that's already on the site — deepen a project write-up from its
code, add the backstory, restructure it — use the **`enhance-post`** skill (`story_enhance` /
`gw story enhance <item>`; in the app: open the post → *✦ Enhance with Claude*). It interviews the
author, reads the repository, and writes a proposal they accept in the app. Small edits (a typo, a
link, a paragraph) are still `update_content_source` on the file directly.

## 6. Finish

- `gw check` [`check_site`] must pass; preview with `gw build --drafts` / `gw serve`.
- Link the idea: `gw idea link <n> <content-id>` and `gw idea set <n> review` [`update_idea`].
- If the material came from somewhere (WordPress, a repo, a book), make sure a registry entry
  exists and set `:source:`.
- Content kept in another repo can be pulled in instead of copied: add it to `sources:` in
  `site.yaml` (repo path + glob); each file needs the usual header.
- Leave it as a draft and tell the user how to preview it. Do not release or publish unasked.


## Tools (`/tools/<slug>/`)

Interactive browser tools are content of kind `tool`: `content/tools/<slug>.adoc` (with `:category:` from `tools.categories` in site.yaml, `:summary:`, `:tags:`, `:network: true` if it fetches anything, optional `:works-with: <slug>: <why>; <slug>: <why>` naming other tools it is genuinely used together with — one line each, no automatic backlinks, so add the reverse entry on the other tool yourself if it applies, worded from its side — and a short "How to use it" body) plus `site/static/tools/<slug>.js` registered with `GW.tool('<slug>', (root, k) => ...)` using the shared kit in `static/tools/tools.js` and the `.t-*` classes in `tools.css`. See docs/authoring.adoc § Tools. Everything must run client-side; network tools may only call CORS-enabled public endpoints and must say which.
