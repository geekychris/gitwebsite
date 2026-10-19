---
name: gitwebsite-code
description: Manage the code catalog on the user's website (gitwebsite) — the /code/ page of their git projects with categories, tags, logos and related projects. Use when the user wants to sync their GitHub repositories to the site, edit or improve a project's description, organise projects into categories or tags, relate projects, get ChatGPT prompts (in batches) for missing project logos, import rendered logos, or add logos to their repositories' READMEs.
---

# gitwebsite-code

The catalog is `code.yaml` beside `site.yaml` (logos in `static/code-logos/`), shown at `/code/`
and `/code/<slug>/`. Everything below works from the CLI (`gw code …`), MCP (`code_*` tools) or
the app's **Code** page. Run `gw site` first to know whose site it is.

## Sync and look

- `gw code sync` [`code_sync`] — reads the owner's repositories with `gh` (must be logged in);
  adds new public repos (forks/archived hidden), refreshes facts, finds logos in READMEs.
- `gw code list [--need-logo]` [`code_list`], `gw code show <slug>`.

## Improve a project's text

Get the context — `code_get` (fields, README, categories, other projects, the house voice) — and
write: a title; a one-sentence summary (≤140 chars); a 2–4 paragraph description (what it is, why
it exists, how it works or what is notable, how to try it; `code`, **bold**, [links](https://…),
`- ` bullets); 3–8 lower-case tags; a category id; up to five related slugs. Never invent features.
Save with `code_update` (`enhanced: true`). Or let the CLI ask Claude: `gw code enhance <slug>`
(shows a proposal) / `--apply`.

## Categories and tags

`gw code suggest` proposes a scheme for the whole catalog; `--apply` fills gaps (`--overwrite`
replaces). By hand: `gw code category <id> --title … --description …`, `gw code set <slug>
--category <id> --tags a,b --related x,y`. MCP: `code_category`, `code_apply_scheme`, `code_update`.

## Logos (ChatGPT batches)

1. `gw code logo-prompts --size 10` [`code_logo_prompts`] prints numbered batches, each item
   naming the file to save as (`<slug>.png`). Give the user one batch at a time to paste into
   ChatGPT. A better summary (improve the text first) gives a better prompt; a project's own
   `logo_prompt` overrides the composed one.
2. The user saves the images; `gw code import-logos <folder or files>` matches them by file name
   (or they drop them on the app's Logos page).
3. Adding logos to the repositories opens pull requests on the user's GitHub — **ask first**, list
   the repositories, then `gw code logo-pr --all --yes` [`code_logo_pr` with `confirm: true`].

Publishing the site (so /code/ changes go live) is the gitwebsite-publish skill — always with the
user's confirmation. Put `{ label: Code, href: /code/ }` in `nav:` in site.yaml for the menu.
