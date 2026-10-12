---
name: suggest-post
description: Proactively suggest sharing finished work on the user's website (a gitwebsite site). Use whenever the user has just finished, shipped, released, or published something notable in ANY repository or session — a release or tag, a published book or document, a finished feature or tool, a new repo made public, an app exposed publicly, a completed physical build (woodworking, 3D printing, electronics, metalwork), or photos of a project — and it has not already been suggested for that piece of work. Offer once to record it as an idea or draft a post; never publish.
---

# suggest-post

The user shares what they make and learn on their website, managed by gitwebsite (`gw site`
shows which site and where). When notable work has just been
finished, offer — once, briefly — to capture it.

## When

After the work is actually done (tests pass, the release is out, the document is published,
the build is finished) — not mid-task. Good triggers: a version tag or GitHub release, a book or
doc published, a new app deployed publicly, a project milestone, a finished physical build,
the user sharing project photos.

Skip it if: the work is trivial or private/confidential (employer work, credentials, client
data); the user already declined for this work; it is already on the site or in the ideas list
(check `gw idea list` / `gw list` or MCP `list_ideas` / `list_content`).

## What to say

One or two lines at the end of your reply, e.g.:

> This would make a good post on your site — want me to add it to the ideas list, or draft a
> project update now?

## If they say yes

- **Record it**: `gw idea add "Title" --kind article|project|update|app --area <area> --source <repo path or URL> --notes "…"`
  [MCP `add_idea`]. Use `gw areas` [`list_areas`] for areas.
- **Draft it**: follow the `gitwebsite-write` skill — create a draft, link it to the idea
  (`gw idea link <n> <content-id>`), and leave it for review.
- If it is a publication or other durable artefact, add a registry entry (`upsert_registry_entry`).

## Never

Nag more than once per piece of work, interrupt unfinished work, or release/publish anything —
publishing goes through `gitwebsite-publish` with explicit confirmation.
