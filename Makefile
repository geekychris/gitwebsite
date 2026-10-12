# gitwebsite
#   make            build the gw binary (release)
#   make site       build a site (no drafts) — SITE=<path to the folder with site.yaml>
#   make preview    build a site including drafts
#   make serve      run the UI + REST + preview at http://127.0.0.1:8790/app/
#   make check      cargo test (+ gw check SITE=… when SITE is set)
#   make app        build the desktop app bundle
#   make install    gw → ~/.local/bin, skills → ~/.claude/skills, app → ~/Applications
#   make docs       render docs/index.adoc to docs/index.html

GW := target/release/gw
# The site to build/serve/check; default: the current site from `gw config`.
SITE ?=
SITE_ARG := $(if $(SITE),--site $(SITE),)

.PHONY: all gw site preview serve check app install install-gw install-skills install-app docs

all: gw

gw:
	cargo build --release -p gw

site: gw
	$(GW) $(SITE_ARG) build

preview: gw
	$(GW) $(SITE_ARG) build --drafts

serve: gw
	$(GW) $(SITE_ARG) serve --open

check: gw
	cargo test -q
	$(if $(SITE),$(GW) $(SITE_ARG) check,)

app:
	cd app/src-tauri && cargo tauri build --bundles app

install: install-gw install-skills install-app

install-gw: gw
	mkdir -p $(HOME)/.local/bin
	ln -sf $(CURDIR)/$(GW) $(HOME)/.local/bin/gw
	@echo "gw → ~/.local/bin/gw (make sure ~/.local/bin is on PATH)"

install-skills:
	scripts/install-skills.sh

install-app: app
	rm -rf $(HOME)/Applications/gitwebsite.app
	mkdir -p $(HOME)/Applications
	cp -R target/release/bundle/macos/gitwebsite.app $(HOME)/Applications/
	@echo "gitwebsite.app → ~/Applications"

docs:
	asciidoctor -o docs/index.html docs/index.adoc

