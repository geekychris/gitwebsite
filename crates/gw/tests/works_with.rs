//! `:works-with:` on tool pages — a "Works with" panel linking the named tools (title, category
//! and the reason), and `gw check`: an unknown tool or a malformed entry is an error, the tool
//! itself or a repeat is a warning.

use std::path::PathBuf;

fn site(name: &str, tools: &[(&str, &str)]) -> PathBuf {
    let root = std::env::temp_dir().join(format!("gw-works-with-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("content/tools")).unwrap();
    std::fs::write(
        root.join("site.yaml"),
        "site:\n  title: T\n  base_url: https://example.org\n  author: A\ntools:\n  categories:\n    - id: math\n      title: Maths & numbers\n    - id: dev\n      title: Developer\n",
    )
    .unwrap();
    for (slug, text) in tools {
        std::fs::write(root.join(format!("content/tools/{slug}.adoc")), text).unwrap();
    }
    root
}

#[test]
fn tool_page_shows_works_with_panel() {
    let root = site(
        "build",
        &[
            ("graph", "= Function grapher\n:type: tool\n:category: math\n:summary: Plot.\n:works-with: units: converts the axes' units; matrix: solves the system <first>; graph: itself; units: again; nope: missing\n\nHow to use it.\n"),
            ("units", "= Unit converter\n:type: tool\n:category: math\n:summary: Units.\n\nHelp.\n"),
            ("matrix", "= Matrix calculator\n:type: tool\n:category: dev\n:summary: Matrices.\n\nHelp.\n"),
            ("draft-tool", "= Secret\n:type: tool\n:status: draft\n:category: dev\n\nHelp.\n"),
            ("plain", "= Plain\n:type: tool\n:category: dev\n:works-with: draft-tool: not built yet\n\nHelp.\n"),
        ],
    );
    let site = gw_core::site::Site::open(Some(&root)).unwrap();
    let out = root.join("out");
    gw_core::build::build(&site, &gw_core::build::BuildOptions { drafts: false, out_dir: Some(out.clone()) }).unwrap();
    let html = std::fs::read_to_string(out.join("tools/graph/index.html")).unwrap();
    let panel = &html[html.find("<section class=\"works-with\"").expect("panel") ..];
    let panel = &panel[..panel.find("</section>").unwrap()];
    assert!(panel.contains("<h2 id=\"works-with-h\">Works with</h2>"), "{panel}");
    // Links in the page's order, with the other tool's title and category title, reasons escaped.
    let units = panel.find("href=\"/tools/units/\"").expect("units link");
    let matrix = panel.find("href=\"/tools/matrix/\"").expect("matrix link");
    assert!(units < matrix, "{panel}");
    assert!(panel.contains("<span class=\"ww-title\">Unit converter</span> <span class=\"ww-cat\">Maths &amp; numbers</span>"), "{panel}");
    assert!(panel.contains("<span class=\"ww-title\">Matrix calculator</span> <span class=\"ww-cat\">Developer</span>"), "{panel}");
    assert!(panel.contains("converts the axes&#39; units"), "{panel}");
    assert!(panel.contains("solves the system &lt;first&gt;"), "{panel}");
    // The tool itself, the repeat and the unknown slug are left out.
    assert_eq!(panel.matches("<li>").count(), 2, "{panel}");
    assert!(!panel.contains("/tools/graph/") && !panel.contains("nope"), "{panel}");
    // After the tool, before "How to use it".
    let (root_at, panel_at, help_at) = (html.find("id=\"tool-root\"").unwrap(), html.find("class=\"works-with\"").unwrap(), html.find("How to use it").unwrap());
    assert!(root_at < panel_at && panel_at < help_at);
    // No panel where nothing resolves (a draft is not built) or nothing is named.
    for t in ["plain", "units"] {
        let h = std::fs::read_to_string(out.join(format!("tools/{t}/index.html"))).unwrap();
        assert!(!h.contains("class=\"works-with\""), "{t}");
    }
    // No backlinks are made: matrix does not list graph.
    let m = std::fs::read_to_string(out.join("tools/matrix/index.html")).unwrap();
    assert!(!m.contains("Works with"));
}

#[test]
fn check_reports_errors_and_warnings() {
    let root = site(
        "check",
        &[
            ("graph", "= Grapher\n:type: tool\n:category: math\n:works-with: units: converts; nope: missing; just-a-slug; units: again; graph: itself\n\nHelp.\n"),
            ("units", "= Units\n:type: tool\n:category: math\n:works-with: graph: plots\n\nHelp.\n"),
        ],
    );
    let site = gw_core::site::Site::open(Some(&root)).unwrap();
    let r = site.check_report().unwrap();
    assert_eq!(r.errors.len(), 2, "{r:?}");
    assert!(r.errors.iter().any(|e| e.contains("tools/graph") && e.contains("no tool \"nope\"")), "{r:?}");
    assert!(r.errors.iter().any(|e| e.contains("tools/graph") && e.contains("just-a-slug")), "{r:?}");
    let ww: Vec<&String> = r.warnings.iter().filter(|w| w.contains(":works-with:")).collect();
    assert_eq!(ww.len(), 2, "{r:?}");
    assert!(ww.iter().any(|w| w.contains("names the tool itself")), "{r:?}");
    assert!(ww.iter().any(|w| w.contains("\"units\" is listed twice")), "{r:?}");
    // check() still lists everything, errors first.
    let all = site.check().unwrap();
    assert_eq!(all.len(), r.errors.len() + r.warnings.len());
    assert_eq!(all[0], r.errors[0]);

    // The CLI: errors fail `gw check` (exit 1); warnings alone do not.
    let gw = env!("CARGO_BIN_EXE_gw");
    let o = std::process::Command::new(gw).arg("--site").arg(&root).arg("check").output().unwrap();
    let stdout = String::from_utf8_lossy(&o.stdout);
    assert_eq!(o.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("error: tools/graph: :works-with: no tool \"nope\""), "{stdout}");
    assert!(stdout.contains("warning: tools/graph: :works-with: names the tool itself"), "{stdout}");
    std::fs::write(root.join("content/tools/graph.adoc"), "= Grapher\n:type: tool\n:category: math\n:works-with: units: converts; graph: itself\n\nHelp.\n").unwrap();
    let o = std::process::Command::new(gw).arg("--site").arg(&root).arg("check").output().unwrap();
    assert_eq!(o.status.code(), Some(0), "{}", String::from_utf8_lossy(&o.stdout));
}

#[test]
fn tool_catalogue_lists_works_with() {
    let root = site(
        "toolcat",
        &[
            ("graph", "= Grapher\n:type: tool\n:category: math\n:works-with: units: converts the units: all of them; bad entry\n\nHelp.\n"),
            ("units", "= Units\n:type: tool\n:category: math\n\nHelp.\n"),
        ],
    );
    let list = gw::toolcat::list(&root, None, None).unwrap();
    let g = list["tools"].as_array().unwrap().iter().find(|t| t["slug"] == "graph").unwrap().clone();
    assert_eq!(g["works_with"], serde_json::json!(["units"]));
    let info = gw::toolcat::info(&root, "graph").unwrap();
    assert_eq!(info["works_with"], serde_json::json!([{"slug": "units", "reason": "converts the units: all of them"}]));
}
