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

## 3-D models (Meshy models repository)

1. **Make web assets** — `gw media add-model <project>/<slug> --download stl|3mf|none`
   [`add_model`] (needs Blender; 10–60 s). It writes a decimated textured GLB (~2–3 MB), a USDZ
   for iOS AR, a poster and the print file into `static/models/<key>/` and registers
   `media:<key>`. Large STLs (>25 MB): add `--download-polycount 400000`. Find models with
   `gw meshy models` [`meshy_models`]. The app: Models → a model → *Add to site*.
2. **Embed** on its own line: `model::media:<key>[Caption, download=true]` (also `ar=`,
   `autorotate=`, `height=480`, `reveal=interaction`). In a story draft: add it with
   `gw story add <story> media:<key> --comment "…"`, then write `model::story:<id>[…]`.
   Or let gitwebsite place it: `gw models add <post> <media-key|project/slug> [--caption …]
   [--after-line N]` [`post_add_model`] (makes the web model first if needed); list a post's
   models with `gw models in <post>` [`post_models`]; take one out with `gw models remove <post>
   <model>|#n` [`post_remove_model`]. The app: the editor's *3-D models* panel, or Models → a
   model → *In posts* / *Add to post…*.
3. Commit `static/models/<key>/` with the post; `gw check` flags unknown or non-model keys.

The site also has a **models gallery** (a page per model at `/models/<project>/<slug>/`) of the
models the user switches on, kept in `site/models.yaml`: `gw models on <project>/<slug>…
[--viewer]` / `--project <p>` / `off …` / `viewer <key> --on|--off`, or the app's *Show on
website* switches. They copy the renders into `static/model-gallery/` (`gw models sync-site` after
hand edits); commit both. The viewer needs a web model (`gw media add-model <key> --download
none`). The grid itself lives in a post — a living article — with `models::gallery[]` on its own
line (`models::gallery[project=<id>]` for one project); `/models/` then redirects there and the
post's updated date follows the newest model added.

## Rules

- Always give images meaningful alt text.
- Do not upload or publish anything the user has not chosen; uploading to SmugMug is
  outward-facing (it can be public depending on album settings) — confirm first.
- After adding media run `gw check` [`check_site`] and preview (`gw build --drafts`).
