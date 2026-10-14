---
name: project-story
description: Run a project interview with the user and turn it into a story for their website (gitwebsite) — they give you photos (pasted images, folders on their Mac, Apple Photos selections or albums, SmugMug album links), choose the ones that matter with a comment each, tell the story roughly, and you interview them, then outline and draft the post. Use when the user says "let's do a project interview", "project interview", "interview me about a project/build/event", "set up a project story", "write up the story of …" from photos, runs /project-story, or names a story slug from the gitwebsite app's Stories section (e.g. "creating the Amiga 40th exhibit").
---

# project-story — the project interview

A story is a workspace in gitwebsite (`site/stories/<slug>/`): candidate photos to choose from,
the photos the author chose (each with **their comment**), "the story so far" in their words,
notes, the interview transcript, and the outline. You run the interview from the terminal; the
author can answer and choose photos **here or in the gitwebsite app** — both edit the same story,
and the app updates live. The `gw` CLI is the interface (`gw story --help`); the `gitwebsite` MCP
server has the same operations as `story_*` tools.

**Arguments:** a story slug (`/project-story amiga-40th-exhibit`), a title for a new one, or
nothing (then gather what the message gives you, below).

## 0. Ground rules

- **Never invent facts.** Everything in the post comes from the author's narrative, notes, photo
  comments, interview answers, or what is plainly visible in a chosen photo. Unsure → ask, or
  leave it out.
- **Their words are primary material.** Comments, narrative, and answers are quoted and built on,
  never contradicted. If a photo seems to disagree with a comment, ask.
- **People:** ask before naming anyone or describing identifiable people in photos. If they say
  not to name someone, don't — anywhere.
- **Nothing goes live from this skill.** You write a draft. Releasing needs their explicit OK;
  publishing needs an explicit request (then use the `gitwebsite-publish` skill).
- **Keep it quick and conversational**: a sentence of reaction, then 2–4 questions. Don't paste
  JSON or long command output at the author — summarise.
- **Commands:** `gw` finds the site itself (`--site <dir>`, `$GW_SITE`, the folder you are in, or the
  current site in `gw config show`); `gw site` says whose site it is and where. Refer to the author
  by the name it gives.

## 1. Gather sources and create the story

From their message, collect what they gave you:

- **Pasted images** — Claude Code shows a pasted image with its file path; use that path.
- **Folders** on their Mac (`~/Pictures/…`), one or several.
- **SmugMug links** — album, gallery, folder, or single-photo URLs.

If something is clearly missing, ask **once** ("Any photos — a folder, or a SmugMug album?").
Then, if there's no story yet, ask quickly for a **title** and what it should become (project
page / article / update to an existing project — AskUserQuestion is good here) and create it:

```console
gw story new "Creating the Amiga 40th exhibit" --target project --area amiga --tag exhibit
```

Add the photos as **candidates** to choose from (nothing is copied until chosen; their folders are
never modified):

```console
gw story add-dir <slug> ~/Pictures/Amiga40 [--recursive] [--since 2025-10-01] [--limit 200]
gw story add-smugmug <slug> https://<user>.smugmug.com/…/n-XXXX      # read-only on SmugMug
gw story add-photos <slug>                      # Apple Photos (macOS): what they have selected there, as chosen photos
gw story add-photos <slug> --album "Name"       # an Apple Photos album, as candidates (gw story photos-albums lists them)
gw story add <slug> /path/to/pasted.png --candidate
```

`add-dir` reports videos it found but doesn't add them (stories use photos).

## 2. Choose the photos (CLI or app)

```console
gw story contact-sheet <slug>                  # candidates, numbered, page 1 of N (prints the JPEG path)
gw story contact-sheet <slug> --page 2
gw story open-sheet <slug>                     # opens it in Preview for the author
gw story show <slug>                           # also prints the app link: http://127.0.0.1:8790/app/#story/<slug>
```

**Open each contact sheet with the Read tool** and describe briefly what you see, by number
("#3–#6 the exhibit tables, #9 the crowd at the door, #12 a close-up of the A1000 board"). Offer
to `open-sheet` it for them too. Then ask which to use — and tell them they can **choose in the
app** instead (the story page's Candidates panel, where each choice needs a comment); changes
show up on both sides live.

Selecting needs **a comment per photo, in their words** (what's happening, why it matters):

```console
gw story select <slug> 3,7-9,12                    # chosen; lists which still need a comment
gw story comment <slug> 7 "The A1000 with its signatures inside the case"
gw story select <slug> 14 --comment "Doors open at 10 — the queue went round the block"
gw story reject <slug> 2,5        ·  gw story unselect <slug> 9
```

Ask for the missing comments conversationally ("For #8 and #9 — what's going on there?") and
record each answer with `gw story comment`. The selection is complete when every chosen photo has
a comment (`gw story show` says so). Then `gw story review-images <slug>` and **look at each
chosen photo** at full review size with the Read tool.

## 3. The story so far

Ask: *"Tell me the story in a few sentences, as rough as you like."* Save it in their words:

```console
gw story narrative <slug> "It started when the Computer History Museum…"     # appends
```

(`--set` replaces; they may also type it in the app's "The story so far" box.)

## 4. Interview in short rounds — answers from either place

**Before every round:** `gw story changes <slug> --since <rev>` (rev from the previous output) —
acknowledge anything the author did in the app ("I see you added #15 and wrote about the setup —
thanks"). Build questions on the chosen photos, comments, and narrative ("In #7 the case is
open — whose signatures are those?").

For each question you're waiting on, record it so the app shows it too:

```console
gw story ask <slug> "How many machines were on show, and who lent them?"
```

Tell them once: **"Answer here, or in the gitwebsite app — whichever's handier."**

- If they answer in the chat: `gw story answer <slug> "<their answer>"` (oldest pending, or
  `--id N`). Keep their wording.
- If they say they answered in the app, or a turn starts with a question still pending: run
  `gw story pending <slug>` / `gw story changes <slug> --since <rev>` — app answers are already in
  the transcript. If they're answering in the app right now, `gw story wait <slug> --answer
  --timeout 300` returns as soon as they send it.
- A fact you just learned without having `ask`ed: `gw story qa <slug> --q "…" --a "…"`.

Cover, skipping what's answered: what/why/for whom · origins and timeline · planning and design ·
the build and technical details · problems and fixes · people (ask how to credit them) · numbers
they know · the day itself / outcome / reception · lessons and what's next · links (event page,
repos, galleries). Stop when there's enough (usually 4–8 rounds) and ask "Anything else?".

## 5. Outline — get approval

Propose title, one-line summary, target, areas, tags, sections, which chosen photos go where (by
number/id), the hero photo, captions, and which chosen photos to leave out. For a big story,
suggest a project page plus dated updates (each its own story: `--target update --project
<slug>`). When agreed: `gw story outline <slug> --file outline.md` and `gw story set <slug> …`.

## 6. Draft, preview, iterate

Write AsciiDoc in **their voice** (first person, concrete, plain; `docs/authoring.adoc` →
"Voice and structure"). Refer to chosen photos as `story:<id>`:

```asciidoc
= Creating the Amiga 40th exhibit
:summary: How twenty-two Amigas and a borrowed hall became a one-day exhibit.
:hero: story:img1

.Every model from the A1000 to the A4000T
image::story:img3[A long table of Amiga computers, each running a demo]
```

```console
gw story draft <slug> --file /tmp/story-draft.adoc
```

Local photos are copied in at web size with EXIF/GPS removed; SmugMug photos become media keys.
Only use chosen photos (`state: selected`) with comments — the draft warns otherwise. Share the
`preview_url` (the app must be running; otherwise `gw serve`) and iterate (re-running `draft`
rewrites the same post).

## 7. Finish

- Ready: `gw release <content-id>` and `gw story set <slug> --status released` (goes live at the
  next publish).
- Publish only if they explicitly ask — use the `gitwebsite-publish` skill.
- Offer to record follow-ups as ideas: `gw idea add "…"`.

## Reference

| Command | MCP tool |
|---|---|
| `gw story list` / `show <slug>` | `story_list` / `story_show` |
| `gw story new "<title>" --target … --area … --tag …` | `story_create` |
| `gw story set <slug> --title/--target/--project/--area/--tag/--status` | `story_update` |
| `gw story add-dir <slug> <dirs…> [--recursive --since --limit]` | `story_add_dir` |
| `gw story add-smugmug <slug> <links…>` | `story_add_smugmug` |
| `gw story add-photos <slug> [--album NAME] [--candidate\|--chosen] [--limit N]`, `photos-albums` | `story_add_photos`, `photos_albums` |
| `gw story add <slug> <files/URLs/refs> [--candidate] [--caption] [--comment]` | `story_add_images` |
| `gw story contact-sheet <slug> [--selected/--rejected/--all] [--page N]`, `open-sheet` | `story_contact_sheet` |
| `gw story select <slug> 3,7-9 [--comment …]`, `unselect`, `reject` | `story_select`, `story_unselect`, `story_reject` |
| `gw story comment <slug> <number/id> "<text>"`, `caption …`, `exclude … [--undo]` | `story_comment`, `story_update_image` |
| `gw story narrative <slug> "<text>" [--set]` | `story_narrative` |
| `gw story note <slug> "<text>"` | `story_note` |
| `gw story ask <slug> "<q>"`, `answer <slug> "<a>" [--id N]`, `pending <slug>` | `story_ask`, `story_answer`, `story_pending` |
| `gw story changes <slug> --since <rev>`, `wait <slug> [--answer]` | `story_changes` |
| `gw story qa <slug> --q … --a …` | `story_qa` |
| `gw story outline <slug> --file/--text`, `draft <slug> --file/--text` | `story_outline`, `story_draft` |
| `gw story review-images <slug>` | `story_review_images` |
| `gw story open-app <slug>` | (the app: `http://127.0.0.1:8790/app/#story/<slug>`) |
| `gw story smugmug-browse <url-or-AlbumKey>`, `smugmug-albums` | `smugmug_browse`, `smugmug_albums` |
