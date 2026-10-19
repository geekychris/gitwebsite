//! The code catalog's REST API end to end (no GitHub or Claude): a scratch site and the real
//! router — editing a project, categories, importing a logo by upload, serving it, and the guards
//! on logo pull requests and logo paths.

use serde_json::{json, Value};

async fn serve(r: axum::Router) -> u16 {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let p = l.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(l, r).await.unwrap() });
    p
}

fn png(w: u32, h: u32) -> Vec<u8> {
    let img = image::RgbImage::from_pixel(w, h, image::Rgb([20, 100, 200]));
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img).write_to(&mut out, image::ImageFormat::Jpeg).unwrap();
    out.into_inner()
}

#[tokio::test]
async fn edit_categorise_and_import_a_logo_over_http() {
    let site = std::env::temp_dir().join(format!("gw-code-api-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&site);
    std::fs::create_dir_all(site.join("content")).unwrap();
    std::fs::write(site.join("site.yaml"), "site:\n  title: T\n  base_url: https://example.org\n  author: A\n").unwrap();
    std::fs::write(site.join("code.yaml"), "owner: me\nprojects:\n  - { slug: amiga-mcp, repo: me/amiga_mcp, github: { description: From GitHub. } }\n  - { slug: other, repo: me/other }\n").unwrap();

    let port = serve(gw::server::router(Some(site.clone()))).await;
    let api = |p: &str| format!("http://127.0.0.1:{port}/api{p}");
    let c = reqwest::Client::new();

    let v: Value = c.get(api("/code")).send().await.unwrap().json().await.unwrap();
    assert_eq!(v["counts"]["listed"], 2);
    assert_eq!(v["projects"][0]["display_summary"], "From GitHub.");
    assert_eq!(v["counts"]["need_logo"], 2);

    // Categories, then a project's fields.
    let r = c.put(api("/code")).json(&json!({ "categories": [{ "id": "Retro Stuff", "title": "Retro" }] })).send().await.unwrap();
    assert!(r.status().is_success());
    let v: Value = c
        .put(api("/code/projects/amiga-mcp"))
        .json(&json!({ "title": "Amiga DevBench", "category": "retro-stuff", "tags": ["Amiga", "MCP"], "related": ["other"], "enhanced": true }))
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(v["display_title"], "Amiga DevBench");
    assert_eq!(v["tags"], json!(["amiga", "mcp"]));
    assert!(v["enhanced"].as_str().is_some_and(|d| d.len() == 10));
    let r = c.put(api("/code/projects/amiga-mcp")).json(&json!({ "category": "nope" })).send().await.unwrap();
    assert!(r.status().is_client_error() || r.status().is_server_error());
    let v: Value = c.get(api("/code/projects/other")).send().await.unwrap().json().await.unwrap();
    assert_eq!(v["related_back"], json!(["amiga-mcp"]));

    // A rendered logo, matched by file name.
    let v: Value = c.post(api("/code/logos/upload?name=Amiga_MCP%20(2).jpg")).body(png(900, 900)).send().await.unwrap().json().await.unwrap();
    assert_eq!(v["slug"], "amiga-mcp");
    let st: Value = c.get(api("/code")).send().await.unwrap().json().await.unwrap();
    let p = st["projects"].as_array().unwrap().iter().find(|p| p["slug"] == "amiga-mcp").unwrap().clone();
    assert_eq!(p["logo_source"], "imported");
    assert_eq!(st["counts"]["awaiting_pr"], 1);
    let img = c.get(format!("http://127.0.0.1:{port}{}", p["logo_url"].as_str().unwrap())).send().await.unwrap();
    assert_eq!(img.headers()["content-type"], "image/png");
    let unmatched = c.post(api("/code/logos/upload?name=nothing.png")).body(png(10, 10)).send().await.unwrap();
    assert!(!unmatched.status().is_success());

    // Guards: no pull requests without confirm; no paths out of the logo folder.
    let r = c.post(api("/code/logo-prs")).json(&json!({ "all": true })).send().await.unwrap();
    assert_eq!(r.status(), 400);
    for bad in ["..%2Fcode.yaml", "amiga-mcp.exe", "..%2F..%2Fsite.yaml"] {
        let r = c.get(api(&format!("/code/logo/{bad}"))).send().await.unwrap();
        assert_eq!(r.status(), 404, "{bad}");
    }

    // The prompts skip the project that has a logo now.
    let v: Value = c.get(api("/code/logo-prompts?size=5")).send().await.unwrap().json().await.unwrap();
    assert_eq!(v["count"], 1);
    assert!(v["batches"][0]["text"].as_str().unwrap().contains("File name: other.png"));
}
