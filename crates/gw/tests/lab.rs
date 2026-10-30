//! The Lab: explorations (a content kind with its own page), `tool::` live views, `:redirect-from:`
//! redirects, `:lab: true` tools, and the Lab page's sections — built, and checked by `gw check`.

use std::path::{Path, PathBuf};

const TOOL_KAFKA: &str = "= Kafka simulator\n:type: tool\n:category: dev\n:summary: Partitions.\n\nHelp.\n\n== Linking to an example\n\nAdd a tab after `#`: `sim` (the default) or `keys`.\n\n[cols=\"2,4\"]\n|===\n| `example=` | Shows\n\n| `wal` | The write-ahead log\n| `unclean` | Unclean election\n|===\n";

fn site(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let root = std::env::temp_dir().join(format!("gw-lab-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("content")).unwrap();
    std::fs::write(
        root.join("site.yaml"),
        "site:\n  title: T\n  base_url: https://example.org\n  author: A\nareas:\n  - id: eng\n    title: Engineering\ntools:\n  categories:\n    - id: dev\n      title: Developer\n",
    )
    .unwrap();
    for (rel, text) in files {
        let p = root.join("content").join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    root
}

fn build(root: &Path) -> (PathBuf, Vec<String>) {
    let site = gw_core::site::Site::open(Some(root)).unwrap();
    let out = root.join("out");
    let r = gw_core::build::build(&site, &gw_core::build::BuildOptions { drafts: false, out_dir: Some(out.clone()) }).unwrap();
    (out, r.warnings)
}

fn read(out: &Path, rel: &str) -> String {
    std::fs::read_to_string(out.join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

fn lab_site(name: &str) -> PathBuf {
    site(
        name,
        &[
            ("pages/home.adoc", "= Home\n\nHello.\n"),
            ("tools/kafka-sim.adoc", TOOL_KAFKA),
            ("tools/sandbox.adoc", "= Language sandbox\n:type: tool\n:category: dev\n:lab: true\n:summary: Run code.\n\nHelp.\n"),
            ("tools/units.adoc", "= Units\n:type: tool\n:category: dev\n:summary: Convert.\n\nHelp.\n"),
            (
                "lab/explorations/replication.adoc",
                "= Replication by hand\n:date: 2026-10-20\n:areas: eng\n:series: Distributed basics\n:series-order: 2\n:summary: Kill a leader.\n:redirect-from: /articles/replication/, /old/replication.html\n\nIntro.\n\ntool::kafka-sim[query=\"example=wal\", tab=sim, height=600, caption=\"Kill the leader\"]\n\nThen convert.\n\ntool::units[]\n\ntool::kafka-sim[query=example=unclean]\n",
            ),
            ("lab/explorations/logs.adoc", "= Logs first\n:date: 2026-10-01\n:series: Distributed basics\n:series-order: 1\n:summary: Start here.\n\nText.\n"),
            ("lab/explorations/solo.adoc", "= Solo\n:date: 2026-10-25\n:summary: Alone.\n\nText.\n"),
            ("lab/playground/lua.adoc", "= Lua basics\n:summary: Lua.\n\n[source.playground,lua]\n----\nprint(1)\n----\n"),
            ("lab/playground/two.adoc", "= Prolog and Scheme\n\n[source.playground,pl]\n----\nx.\n----\n\n[source.playground,scheme]\n----\n(+ 1 2)\n----\n"),
            ("lab/sketches/waves.adoc", "= Waves\n\n[source.playground,glsl]\n----\nvoid main(){}\n----\n"),
            ("lab/playground/dots.adoc", "= Dots\n:tags: sketch\n\n[source.playground,p5]\n----\n----\n"),
            ("lab/rosetta/fizzbuzz.adoc", "= FizzBuzz\n\n[source.playground,python]\n----\nprint(1)\n----\n"),
            ("articles/a.adoc", "= An article\n:date: 2026-10-10\n:summary: Words.\n\ntool::units[caption=Inline in an article]\n"),
        ],
    )
}

#[test]
fn exploration_page_tool_views_and_series() {
    let root = lab_site("build");
    let (out, warnings) = build(&root);
    assert!(warnings.is_empty(), "{warnings:?}");
    let h = read(&out, "lab/explorations/replication/index.html");
    assert!(h.contains("<a href=\"/lab/\">Lab</a> · Exploration"), "eyebrow");
    assert!(h.contains("min read"));
    // The tools it uses, each once, linked, in order of appearance.
    let uses = &h[h.find("class=\"xp-uses\"").expect("uses")..];
    let uses = &uses[..uses.find("</p>").unwrap()];
    assert!(uses.find("href=\"/tools/kafka-sim/\">Kafka simulator").unwrap() < uses.find("href=\"/tools/units/\">Units").unwrap(), "{uses}");
    assert_eq!(uses.matches("<a ").count(), 2, "{uses}");
    // Frames: lazy, bare, query and tab, height, caption and the full-tool link.
    assert!(h.contains("<iframe src=\"/tools/kafka-sim/?bare=1&amp;embed=1&amp;example=wal#sim\" title=\"Kafka simulator (live tool)\" loading=\"lazy\""), "{h}");
    assert!(h.contains("--te-h:600px"));
    assert!(h.contains("Kill the leader</span><a class=\"te-open\" href=\"/tools/kafka-sim/?example=wal#sim\">Open full tool →</a>"));
    assert!(h.contains("<iframe src=\"/tools/units/?bare=1&amp;embed=1\""));
    assert_eq!(h.matches("<figure class=\"tool-embed\"").count(), 3);
    assert_eq!(h.matches(".tool-embed{").count(), 1, "styles once per page");
    // Series: part 2 of 2, previous is "Logs first", no next.
    assert!(h.contains("Part 2 of 2 in <strong>Distributed basics</strong>"));
    assert!(h.contains("href=\"/lab/explorations/logs/\" rel=\"prev\""));
    assert!(!h.contains("rel=\"next\""));
    let logs = read(&out, "lab/explorations/logs/index.html");
    assert!(logs.contains("href=\"/lab/explorations/replication/\" rel=\"next\"") && !logs.contains("rel=\"prev\""));
    assert!(!read(&out, "lab/explorations/solo/index.html").contains("class=\"xp-nav\""));
    // tool:: works in any page.
    assert!(read(&out, "articles/a/index.html").contains("<iframe src=\"/tools/units/?bare=1&amp;embed=1\""));
    // Explorations are in the feed, sitemap, search and their areas.
    assert!(read(&out, "feed.xml").contains("/lab/explorations/replication/"));
    assert!(read(&out, "sitemap.xml").contains("/lab/explorations/replication/"));
    assert!(read(&out, "search.json").contains("\"kind\":\"Exploration\""));
    assert!(read(&out, "areas/eng/index.html").contains("/lab/explorations/replication/"));
    // The theme's tool page hides everything but the tool in bare mode.
    assert!(read(&out, "tools/units/index.html").contains("gw-bare"));
}

#[test]
fn redirects_from_old_addresses() {
    let root = lab_site("redirects");
    let (out, _) = build(&root);
    for rel in ["articles/replication/index.html", "old/replication.html"] {
        let r = read(&out, rel);
        assert!(r.contains("<meta http-equiv=\"refresh\" content=\"0; url=/lab/explorations/replication/\">"), "{rel}: {r}");
        assert!(r.contains("<link rel=\"canonical\" href=\"/lab/explorations/replication/\">"));
        assert!(r.contains("<a href=\"/lab/explorations/replication/\">Replication by hand</a>"));
    }
    for f in ["sitemap.xml", "feed.xml", "search.json"] {
        assert!(!read(&out, f).contains("/articles/replication/"), "{f}");
        assert!(!read(&out, f).contains("old/replication.html"), "{f}");
    }
}

#[test]
fn lab_page_sections() {
    let root = lab_site("labpage");
    let (out, _) = build(&root);
    let h = read(&out, "lab/index.html");
    let at = |s: &str| h.find(s).unwrap_or_else(|| panic!("no {s}"));
    // The statement, with a link to Tools.
    assert!(h.contains("The Lab is for exploring") && h.contains("<a href=\"/tools/\">Tools</a> are for getting a job done"));
    // Sections in order.
    let order = ["id=\"explorations\"", "id=\"playgrounds\"", "id=\"sketches\"", "id=\"rosetta\"", "id=\"lab-tools\""];
    let pos: Vec<usize> = order.iter().map(|s| at(s)).collect();
    assert!(pos.windows(2).all(|w| w[0] < w[1]), "{pos:?}");
    assert!(!h.contains("id=\"apps\""), "empty sections are hidden");
    // Explorations newest first, with series and the tools they use.
    assert!(at("/lab/explorations/solo/") < at("/lab/explorations/replication/") && at("/lab/explorations/replication/") < at("/lab/explorations/logs/"));
    assert!(h.contains("Distributed basics · part 2"));
    // Playgrounds by language: the language list and each card's languages; sketches and Rosetta apart.
    let pg = &h[at("id=\"playgrounds\"")..at("id=\"sketches\"")];
    assert!(pg.contains("data-lang=\"lua\"") && pg.contains("data-lang=\"prolog\"") && pg.contains("data-lang=\"scheme\""), "{pg}");
    assert!(pg.contains("data-langs=\"prolog scheme \"") && pg.contains("Prolog · Scheme"), "{pg}");
    assert!(!pg.contains("Waves") && !pg.contains("Dots") && !pg.contains("FizzBuzz"));
    let sk = &h[at("id=\"sketches\"")..at("id=\"rosetta\"")];
    assert!(sk.contains("/lab/sketches/waves/") && sk.contains("/lab/playground/dots/") && sk.contains("GLSL"), "{sk}");
    assert!(h[at("id=\"rosetta\"")..].contains("/lab/rosetta/fizzbuzz/"));
    // :lab: tools, which stay under /tools/.
    let lt = &h[at("id=\"lab-tools\"")..];
    assert!(lt.contains("Notebooks and sandboxes in Tools") && lt.contains("href=\"/tools/sandbox/\"") && !lt.contains("/tools/units/"), "{lt}");
    assert!(out.join("tools/sandbox/index.html").exists());
    // Tools index points to the Lab; the home page's Lab section shows explorations.
    assert!(read(&out, "tools/index.html").contains("see the <a href=\"/lab/\">Lab</a>"));
    assert!(read(&out, "index.html").contains("Explore the Lab →"));

    // An empty Lab says so and shows no section.
    let empty = site("labempty", &[("articles/a.adoc", "= A\n:date: 2026-10-10\n\nx\n")]);
    let (out, _) = build(&empty);
    let h = read(&out, "lab/index.html");
    assert!(h.contains("Nothing in the Lab yet.") && !h.contains("section-title"));
}

#[test]
fn check_reports_tool_views_and_redirects() {
    let root = site(
        "check",
        &[
            ("tools/kafka-sim.adoc", TOOL_KAFKA),
            ("tools/units.adoc", "= Units\n:type: tool\n:category: dev\n\nHelp.\n"),
            ("articles/taken.adoc", "= Taken\n:date: 2026-10-10\n\nx\n"),
            (
                "lab/explorations/x.adoc",
                "= X\n:redirect-from: /articles/taken/, /lab/, /fine/, /lab/explorations/x/\n\ntool::kafka-sim[query=example=wal]\n\ntool::kafka-sim[query=example=nope, tab=log]\n\ntool::kafka[]\n\ntool::units[query=example=anything, tab=whatever, colour=red]\n",
            ),
            ("articles/b.adoc", "= B\n:date: 2026-10-11\n:redirect-from: /fine/\n:lab: true\n\nx\n"),
        ],
    );
    let site = gw_core::site::Site::open(Some(&root)).unwrap();
    let r = site.check_report().unwrap();
    let has = |v: &[String], s: &str| v.iter().any(|e| e.contains(s));
    assert!(has(&r.errors, "tool::kafka-sim: \"kafka-sim\" has no example \"nope\" (it has wal, unclean)"), "{r:?}");
    assert!(has(&r.errors, "no tool \"kafka\" (content/tools/kafka.adoc) — did you mean kafka-sim?"), "{r:?}");
    assert!(has(&r.errors, ":redirect-from: /articles/taken/ is already the page of articles/taken"), "{r:?}");
    assert!(has(&r.errors, ":redirect-from: /lab/ is already the generated page /lab/"), "{r:?}");
    assert!(has(&r.errors, ":redirect-from: /lab/explorations/x/ is this page's own address"), "{r:?}");
    assert!(has(&r.errors, ":redirect-from: /fine/ is already a redirect to"), "{r:?}");
    assert_eq!(r.errors.len(), 6, "{:#?}", r.errors);
    // Warnings: a tab the tool does not list, an unknown attribute, no :date:, :lab: off a tool.
    assert!(has(&r.warnings, "tab \"log\" is not one the tool lists (sim, keys)"), "{r:?}");
    assert!(has(&r.warnings, "unknown attribute \"colour\""), "{r:?}");
    assert!(has(&r.warnings, "explorations need a :date:"), "{r:?}");
    assert!(has(&r.warnings, "articles/b: :lab: only lists tool pages"), "{r:?}");

    // The CLI fails on the errors.
    let o = std::process::Command::new(env!("CARGO_BIN_EXE_gw")).arg("--site").arg(&root).arg("check").output().unwrap();
    assert_eq!(o.status.code(), Some(1), "{}", String::from_utf8_lossy(&o.stdout));
}

#[test]
fn gw_new_exploration() {
    let root = site("new", &[]);
    let o = std::process::Command::new(env!("CARGO_BIN_EXE_gw"))
        .arg("--site")
        .arg(&root)
        .args(["new", "exploration", "Consistency by hand", "--area", "eng"])
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let text = std::fs::read_to_string(root.join("content/lab/explorations/consistency-by-hand.adoc")).unwrap();
    assert!(text.starts_with("= Consistency by hand\n:type: exploration\n:date: "), "{text}");
    assert!(text.contains("tool::<slug>["));
    let o = std::process::Command::new(env!("CARGO_BIN_EXE_gw")).arg("--site").arg(&root).args(["list", "--kind", "exploration"]).output().unwrap();
    assert!(String::from_utf8_lossy(&o.stdout).contains("lab/explorations/consistency-by-hand"));
}
