//! `:hero-crop:` — a card shows the chosen part of its hero image (inline styles, so any site
//! stylesheet works); a bad value is a check warning and changes nothing.

#[test]
fn cards_show_the_cropped_part_of_the_hero() {
    let root = std::env::temp_dir().join(format!("gw-hero-crop-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("content/articles")).unwrap();
    std::fs::write(root.join("site.yaml"), "site:\n  title: T\n  base_url: https://example.org\n  author: A\n").unwrap();
    std::fs::write(root.join("content/articles/a.adoc"), "= Cropped\n:type: article\n:date: 2026-01-02\n:hero: https://img.example/a.jpg\n:hero-crop: 10, 20, 50, 40\n\nText.\n").unwrap();
    std::fs::write(root.join("content/articles/b.adoc"), "= Plain\n:type: article\n:date: 2026-01-01\n:hero: https://img.example/b.jpg\n:hero-crop: 90, 0, 50, 40\n\nText.\n").unwrap();

    let site = gw_core::site::Site::open(Some(&root)).unwrap();
    let warnings = site.check().unwrap();
    let bad: Vec<&String> = warnings.iter().filter(|w| w.contains(":hero-crop:")).collect();
    assert!(bad.len() == 1 && bad[0].contains("articles/b"), "{warnings:?}");
    let out = root.join("out");
    gw_core::build::build(&site, &gw_core::build::BuildOptions { drafts: false, out_dir: Some(out.clone()) }).unwrap();
    let html = std::fs::read_to_string(out.join("articles/index.html")).unwrap();
    let a = &html[html.find("img.example/a.jpg").unwrap() - 200..];
    assert!(a.contains("aspect-ratio:16/9"), "{a}");
    assert!(a.contains("width:200%;height:250%;left:-20%;top:-50%"), "{}", &a[..600]);
    // An invalid crop is ignored.
    let b = &html[html.find("img.example/b.jpg").unwrap() - 120..html.find("img.example/b.jpg").unwrap() + 80];
    assert!(!b.contains("style="), "{b}");
}
