//! PUT/GET /api/site-models end to end: a scratch config dir (GW_CONFIG_DIR) pointing at a fake
//! models repository, a scratch site, and the real router. Its own test binary, so the
//! process-wide GW_CONFIG_DIR does not leak into other tests.

use serde_json::{json, Value};

async fn serve(r: axum::Router) -> u16 {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let p = l.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(l, r).await.unwrap() });
    p
}

#[tokio::test]
async fn toggles_over_http_and_refuses_bad_keys() {
    let base = std::env::temp_dir().join(format!("gw-site-models-api-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let (cfg, site, repo) = (base.join("config"), base.join("site"), base.join("repo"));
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::create_dir_all(site.join("content")).unwrap();
    std::fs::write(site.join("site.yaml"), "site:\n  title: T\n  base_url: https://example.org\n  author: A\n").unwrap();
    std::fs::write(site.join("media.yaml"), "media:\n  wiz-web:\n    url: /models/wiz-web/wiz.glb\n    kind: model\n    provider: local\n    ids:\n      model: lemmings/wiz\n").unwrap();
    for s in ["wiz", "gino"] {
        let d = repo.join("lemmings").join(s);
        std::fs::create_dir_all(d.join("images")).unwrap();
        std::fs::write(d.join("meshy.json"), format!(r#"{{"name":"{s}","slug":"{s}","project":"lemmings"}}"#)).unwrap();
        std::fs::write(d.join("images/hero.webp"), "h").unwrap();
    }
    std::fs::create_dir_all(repo.join("tools")).unwrap();
    std::fs::write(repo.join("tools/sync"), "").unwrap();
    std::fs::write(cfg.join("config.toml"), format!("[meshy]\nrepo = {:?}\nenabled = false\ninterval_hours = 24\n", repo.display().to_string())).unwrap();
    std::env::set_var("GW_CONFIG_DIR", &cfg);

    let port = serve(gw::server::router(Some(site.clone()))).await;
    let url = format!("http://127.0.0.1:{port}/api/site-models");
    let c = reqwest::Client::new();

    let v: Value = c.get(&url).send().await.unwrap().json().await.unwrap();
    assert_eq!(v["count"], 0);
    assert_eq!(v["exists"], false);

    let r = c.put(&url).json(&json!({"changes": [{"key": "lemmings/wiz", "on": true, "viewer": true}]})).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["count"], 1);
    assert_eq!(v["models"][0]["media_key"], "wiz-web");
    assert_eq!(v["web_models"]["lemmings/wiz"], "wiz-web");
    assert!(site.join("models.yaml").is_file());
    assert!(site.join("static/model-gallery/lemmings/wiz/hero.webp").is_file());

    let r = c.put(&url).json(&json!({"changes": [{"project": "lemmings", "on": true}]})).send().await.unwrap();
    assert_eq!(r.json::<Value>().await.unwrap()["count"], 2);

    for bad in ["../repo/lemmings", "lemmings/../../../etc", "lemmings/missing", "lemmings", "/etc/passwd", "lemmings/.hidden"] {
        let r = c.put(&url).json(&json!({"changes": [{"key": bad, "on": true}]})).send().await.unwrap();
        assert_eq!(r.status(), 400, "{bad}");
        assert!(r.json::<Value>().await.unwrap()["error"].is_string());
    }
    let r = c.put(&url).json(&json!({"changes": [{"project": "..", "on": true}]})).send().await.unwrap();
    assert_eq!(r.status(), 400);
    // Nothing escaped the site.
    assert!(!base.join("etc").exists() && !site.join("static/model-gallery/..").join("etc").exists());

    let r = c.put(&url).json(&json!({"changes": [{"key": "lemmings/wiz", "on": false}]})).send().await.unwrap();
    assert_eq!(r.json::<Value>().await.unwrap()["count"], 1);
    assert!(!site.join("static/model-gallery/lemmings/wiz").exists());
    let _ = std::fs::remove_dir_all(&base);
}
