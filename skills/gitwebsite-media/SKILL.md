---
name: gitwebsite-media
description: Add images and video to the user's website (gitwebsite) — files beside a post, external URLs, the media library (media:<key>), and SmugMug-hosted photos and video. Use when the user wants to add photos or a video to a post, upload media to SmugMug for the site, embed an image from SmugMug or elsewhere, or manage the site's media library.
---

# gitwebsite-media

Whose site and where: run `gw site` (or the MCP `site_info` tool) first — it gives the site's
title, author, address (`base_url`), folder, and publish target. Refer to the author by name
from there; never assume one. `gw` finds the site itself (`--site <dir>`, `$GW_SITE`, the folder
you are in, or the current site from `gw config show`).

Three ways to put media in a post:

1. **Beside the post** — make the post a bundle (`gw new article "…" --bundle`) and drop files
   next to its `index.adoc`: `image::bench.jpg[Alt]`. Good for a few small images. Keep files
   reasonably sized (resize to ~2000 px wide, `sips -Z 2000 file.jpg`): they go into the Pages repo.
2. **External URL** — `image::https://…[Alt]` directly in the text.
3. **Media library** — a named key usable anywhere as `media:<key>`:
   `gw media add bench-front https://… --alt "Bench, front"` [`add_media`], then
   `image::media:bench-front[]`, `video::media:demo[]`, or `:hero: media:bench-front`.
   `gw media list` [`list_media`]. Unknown keys fail `gw check`, so typos never ship.

## SmugMug (large photo sets and video)

SmugMug hosts originals and serves resized images and video, so big media stays out of the
Pages repo. Intended flow with `gw smugmug …` (subcommands provided by the integrations
module; run `gw smugmug --help` for the exact list):

1. **Credentials** — the user runs `gw smugmug auth` **in their own terminal** (it opens an
   authorisation URL and stores tokens in the macOS Keychain). Never ask for keys, secrets,
   or passwords in chat.
2. **Upload** a file or folder to an album; each upload is registered in the media library
   under a key with SmugMug's served URL (and the album recorded in the registry).
3. **Embed** with `image::media:<key>[Alt]` / `video::media:<key>[]`, or use a key as `:hero:`.
4. Existing SmugMug images can be registered by URL with `gw media add` as well.

## Rules

- Always give images meaningful alt text.
- Do not upload or publish anything the user has not chosen; uploading to SmugMug is
  outward-facing (it can be public depending on album settings) — confirm first.
- After adding media run `gw check` [`check_site`] and preview (`gw build --drafts`).
