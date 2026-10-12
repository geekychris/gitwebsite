---
name: gitwebsite-curate
description: Curate the user's website backlog with gitwebsite — review the ideas list and the provenance registry, decide what to write next, move ideas through their statuses, and find finished work that is not yet recorded (recent GitHub repos, releases, published documents). Use when the user asks what to post next, wants to review or groom the ideas/tasks list, plan content for the site, or check what work is missing from the site or registry.
---

# gitwebsite-curate

Whose site and where: run `gw site` (or the MCP `site_info` tool) first — it gives the site's
title, author, address (`base_url`), folder, and publish target. Refer to the author by name
from there; never assume one. `gw` finds the site itself (`--site <dir>`, `$GW_SITE`, the folder
you are in, or the current site from `gw config show`).

Stores: `ideas.yaml` (curation
list), `registry.yaml` (provenance: what exists and where it came from), `media.yaml`.

## Review

1. `gw idea list` [`list_ideas`] — statuses: `idea → planned → drafting → review → published`
   (or `dropped`). Group by status; flag stale items (`updated` long ago).
2. `gw list --drafts` [`list_content` drafts_only] — drafts not yet linked to an idea.
3. `gw registry list` [`list_registry`] — publications and sources; check URLs still resolve
   (`curl -sI`), titles and descriptions are current.

## Find unrecorded work

- `gh repo list <owner> --limit 100 --json name,description,visibility,pushedAt,isPrivate`
  — recently pushed public repos not mentioned in content or the registry.
- `gh release list -R <owner>/<repo>` for recent releases (the owner from the registry's repo
  entries or the site's links).
- Published documents under the site's address (`base_url`; books, decks) missing from the registry.
- Only suggest public work; never surface private repos' contents on the site without asking.

## Propose

Give a short ranked list: what to write next and why (fresh, finished, visual, fills an empty
area). For each, the kind, areas, and source. Then, with the user's agreement:

- add ideas: `gw idea add "…" --kind … --area … --source …` [`add_idea`]
- move status: `gw idea set <n> planned|drafting|review|published|dropped` [`update_idea`]
- add registry entries: `gw registry put entry.yaml` [`upsert_registry_entry`]
- start drafts via the `gitwebsite-write` skill.

Curating changes local files only. Publishing is separate and needs explicit confirmation
(`gitwebsite-publish`).
