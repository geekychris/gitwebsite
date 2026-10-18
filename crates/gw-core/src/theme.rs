//! The built-in default theme: templates and the static assets they use.
//!
//! A site needs no `templates/` or theme assets of its own: the build starts from these and a
//! site's own `templates/<name>` or `static/<path>` replaces the built-in file of the same name.
//! `gw init` copies them into a new site so they are easy to find and change.

/// `(name, text)` for every built-in template.
pub const TEMPLATES: &[(&str, &str)] = &[
    ("404.html", include_str!("../theme/templates/404.html")),
    ("app.html", include_str!("../theme/templates/app.html")),
    ("area.html", include_str!("../theme/templates/area.html")),
    ("areas.html", include_str!("../theme/templates/areas.html")),
    ("article.html", include_str!("../theme/templates/article.html")),
    ("base.html", include_str!("../theme/templates/base.html")),
    ("feed.xml", include_str!("../theme/templates/feed.xml")),
    ("home.html", include_str!("../theme/templates/home.html")),
    ("lab.html", include_str!("../theme/templates/lab.html")),
    ("list.html", include_str!("../theme/templates/list.html")),
    ("model.html", include_str!("../theme/templates/model.html")),
    ("models.html", include_str!("../theme/templates/models.html")),
    ("macros.html", include_str!("../theme/templates/macros.html")),
    ("page.html", include_str!("../theme/templates/page.html")),
    ("playground.html", include_str!("../theme/templates/playground.html")),
    ("project.html", include_str!("../theme/templates/project.html")),
    ("publications.html", include_str!("../theme/templates/publications.html")),
    ("sitemap.xml", include_str!("../theme/templates/sitemap.xml")),
    ("tool.html", include_str!("../theme/templates/tool.html")),
    ("tools.html", include_str!("../theme/templates/tools.html")),
];

/// `(path under static/, bytes)` for every built-in asset.
pub const STATIC: &[(&str, &[u8])] = &[
    ("css/site.css", include_bytes!("../theme/static/css/site.css")),
    ("css/models.css", include_bytes!("../theme/static/css/models.css")),
    ("images/favicon.svg", include_bytes!("../theme/static/images/favicon.svg")),
    ("js/models.js", include_bytes!("../theme/static/js/models.js")),
    ("js/playground-basic.js", include_bytes!("../theme/static/js/playground-basic.js")),
    ("js/playground.js", include_bytes!("../theme/static/js/playground.js")),
    ("js/site.js", include_bytes!("../theme/static/js/site.js")),
    ("tools/tools.css", include_bytes!("../theme/static/tools/tools.css")),
    ("tools/tools.js", include_bytes!("../theme/static/tools/tools.js")),
];
