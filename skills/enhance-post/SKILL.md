---
name: enhance-post
description: Rework an existing post on the user's website (built with gitwebsite) with them — an article, project page, project update, or page — by reading the post and the code repository behind it, interviewing them (inspiration, why, how it works, the hard parts, what's next), and writing a revised version they review and accept in the gitwebsite app. Use when the user says "enhance this post/article/project", "rework the X project page", "improve the write-up of X", "make the article about X better", runs /enhance-post, or names an enhancement story (enhance-<slug>) from the gitwebsite app. Also what the app's "Start Claude" button runs.
---

# enhance-post — rework an existing post

An enhancement is a gitwebsite story tied to an existing content item (`site/stories/enhance-<slug>/`).
It holds the author's answers to a few **key questions**, the interview transcript, the code
repository behind the post (URL and a local checkout, when found), and — once you write it — a
**proposal**: the full revised post. The post itself changes only when the author **accepts** the
proposal in the gitwebsite app. You never accept, release, or publish.

The author works in the gitwebsite app (`http://127.0.0.1:8790/app/#story/<slug>`): they answer
the key questions, answer your questions as they appear, read your proposal as a diff, and accept
it or send it back with what to change. The `gitwebsite` MCP server has every operation (`story_*`
tools); the `gw` CLI has the same ones (`gw story --help`).

**Arguments:** an enhancement slug (`/enhance-post enhance-my-first-post`), or a post (id, slug,
URL, or title) — then start one with `story_enhance` (`gw story enhance <item>`).

## 0. Ground rules

- **Never invent facts.** Everything new comes from the current post, the repository (code,
  README, history), the author's answers, or their notes. Unsure → ask, or leave it out. Don't
  guess dates, numbers, motivations, or people.
- **Their voice.** Keep how they write (first person, plain, concrete — `docs/authoring.adoc` →
  "Voice and structure"). Improve structure, clarity, and depth; don't make it sound like
  marketing.
- **Keep what's there** unless they ask: links, images (`image::…[]` lines, `:hero:`), code
  blocks, credits, and every fact. Removing or replacing something needs a reason you can state.
- **Nothing goes live from this skill.** You write a proposal. Accepting is the author's (in the
  app, or `story_accept` only if they tell you to in the chat). Publishing needs an explicit request
  and the `gitwebsite-publish` skill.
- **People:** ask before naming anyone who isn't already named in the post.
- **Short rounds:** 2–4 questions at a time, each building on what you've read.

## Projects (the main use)

Most enhancements are **project pages** (`content/projects/<slug>/index.adoc`): the page that
introduces a body of work, with dated updates beside it in `updates/`.

- The repository comes from the project's `:repo:` or its registry entry (`repo-<slug>`, whose
  `content` is the project); the local checkout is found by its git remote (e.g.
  a project page whose `:repo:` is `github.com/octocat/hello` → `~/code/hello`, or a submodule
  checkout inside another repository). If `story_context` shows the wrong one or none, ask the author
  and `story_set_repo`.
- **Read the code properly** — this is where a project page gains most: what it is and does,
  how it's built (architecture, the interesting modules, the tricky parts), how to run or try it,
  where it stands (from the history), with short accurate code excerpts.
- Read the project's **updates** (`list_content` with `kind: update`, then `get_content`) and
  link to them instead of repeating them; the project page is the overview, updates are the
  diary. Never edit updates from here — an enhancement changes only the project page, and its
  `updates/` folder is left untouched on accept.
- Keep `:repo:`, `:featured:`, `:hero:`, `:source:`, areas and tags (accept carries them over
  even if you leave them out).
- A good shape: what it is (one paragraph) · why it exists (inspiration, the problem) · what it
  does · how it works · the hard parts · status and what's next · links (repo, live app, updates).

## 1. Read everything first

1. `story_context` (`gw story context <slug>`): the post's current AsciiDoc, the key questions with
   any answers, notes, the interview so far, pending questions, any proposal, and the repository:
   URL, local path, README, last 40 commits, first commit, top-level files.
2. **The repository**, when there's a local path: read the README, then the code that matters —
   the entry points and the modules the post talks about (or should). `git log` for the
   timeline (`git log --reverse --format='%ad %s' --date=short | head`, `git log --stat -5`).
   Note what the post gets wrong, leaves out, or could show (a design decision, a clever bit, a
   number). No local checkout → work from the URL and the post, and ask the author for a path
   (`story_set_repo`).
3. The rest of the site as needed: `get_content` for related posts (the project page for an update,
   updates for a project) so you don't repeat or contradict them.

Then say in a few sentences what you found and what you think the post could gain.

## 2. Interview — answers come from the app

Start from the **key questions** the author hasn't answered (inspiration, why, how it works,
hardest part, what they learned, what's next, who it's for) and from what the code made you
curious about ("The commit log shows the renderer was rewritten in March — what went wrong with
the first one?").

Ask each question with `story_ask` so it shows in the app. Then:

- **Started from the app** (the prompt says so; nobody is at a terminal): wait with
  `story_wait` (`answer_only: true`, `timeout: 240`) and loop until the answers arrive; after about
  15 minutes with nothing, save what you've learned with `story_note` (facts, file:line pointers,
  where you are in the steps), leave the questions pending, and end the session. **The app starts
  you again as soon as the author answers**, so don't wait longer. Never ask anything in plain text.
- **Resuming:** read the story first (`story_show`: notes, outline, the interview with every
  answer, any proposal and feedback) and carry on from where it stopped. Never re-ask an answered
  question or redo an agreed outline; if the last answer approves the plan, write the proposal.
- **Shell:** read the repo with Read, Grep and Glob, and simple read-only commands (`git log`,
  `git show`, `ls`, `head`, `wc`). Anything else is refused when you were started from the app.
- **In a chat:** tell them once "answer here or in the app". Chat answers: `story_answer` (their
  wording). App answers are already in the transcript (`story_changes`, `story_pending`).

Facts you learn without asking: `story_qa`. Usually 2–4 rounds are enough; finish with "Anything
else before I write the revision?"

## 3. Plan the changes — get agreement

Propose what you'll change as a short list: new or reworked sections, what you'll add from the
code (with where it comes from), what you'll cut or tighten and why, title/summary changes, any
new images or code samples. Save it with `story_outline`. Ask (`story_ask`) "Shall I write it like
this, or change the plan?" and wait. Skip this only if the author said "just do it".

## 4. Write the proposal

Write the **complete** revised post — header and body — and save it with `story_draft`
(`gw story draft <slug> --file …`). For an enhancement that stores a proposal; the post is not
touched.

- Start from the current text; keep its header attributes. On accept, `:status:`, `:date:`,
  `:type:`, `:project:`, `:source:`, and `:slug:` always come from the post, any attribute you
  left out is carried over, and `:updated:` is stamped — so you can't move or unpublish it by
  accident.
- Keep existing image and link lines. New photos the author added to the story are
  `image::story:<id>[Alt]`.
- AsciiDoc conventions as in the `gitwebsite-write` skill; code from the repo goes in
  `[source,<lang>]` blocks, short and accurate (copy, don't paraphrase code).

Then tell the author it's ready to review in the app ("Review it in the app — accept, or tell me
what to change"). Started from the app: ask it as a question (`story_ask`: "The proposal is ready
in the app. Accept it there, or tell me here what to change.") and wait.

## 5. Revise

Feedback arrives as an answer, or as a "What should change in proposal N?" entry in the
transcript (the app's *Ask for changes*). Revise and `story_draft` again (it replaces the
proposal). Repeat until they accept or discard it in the app; then stop. If the story shows no
proposal and status `released`, it was accepted — finish with a one-line summary.

## Reference

| Command | MCP tool |
|---|---|
| `gw story enhance <item>` | `story_enhance` |
| `gw story context <slug>` | `story_context` |
| `gw story key <slug> <id> "<answer>"` | `story_key_answers` |
| `gw story repo <slug> --url … --path …` | `story_set_repo` |
| `gw story ask <slug> "<q>"`, `answer`, `pending`, `wait <slug> --answer` | `story_ask`, `story_answer`, `story_pending`, `story_wait` |
| `gw story changes <slug> --since <rev>` | `story_changes` |
| `gw story qa <slug> --q … --a …`, `note`, `outline` | `story_qa`, `story_note`, `story_outline` |
| `gw story draft <slug> --file …` (a proposal) | `story_draft` |
| `gw story proposal <slug>` (diff) | `story_proposal` |
| `gw story request-changes <slug> "<text>"` | `story_request_changes` |
| `gw story accept <slug>` / `discard <slug>` — **the author's call** | `story_accept` (confirm) / `story_discard` |
| `gw story claude <slug> start|stop|status` | (the app's Start Claude button) |
