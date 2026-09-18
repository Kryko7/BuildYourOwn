//! Static file serving for the prerendered SvelteKit site: directory indexes,
//! trailing-slash redirects, extensionless pages and an SPA-safe fallback.
//!
//! The resolution is a pure function of (root, url path) so it can be unit-tested without
//! a socket.

use std::path::{Path, PathBuf};

/// What a URL path resolves to inside the site root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved {
    /// Send this file.
    File(PathBuf),
    /// Send a 301 to this URL (a directory addressed without its trailing slash).
    Redirect(String),
    /// Nothing matched, not even a fallback `index.html`.
    NotFound,
}

/// Resolve a URL path against the site root.
pub fn resolve(root: &Path, url_path: &str) -> Resolved {
    let trailing_slash = url_path.ends_with('/');
    let mut parts: Vec<String> = Vec::new();
    for seg in url_path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                // Never escape the root; treat `..` as a dead end rather than climbing out.
                return Resolved::NotFound;
            }
            s if s.contains('\0') => return Resolved::NotFound,
            s => parts.push(s.to_string()),
        }
    }
    let candidate = parts.iter().fold(root.to_path_buf(), |p, s| p.join(s));

    if candidate.is_file() {
        return Resolved::File(candidate);
    }
    if candidate.is_dir() {
        let index = candidate.join("index.html");
        if index.is_file() {
            if !trailing_slash && !parts.is_empty() {
                return Resolved::Redirect(format!("/{}/", parts.join("/")));
            }
            return Resolved::File(index);
        }
    }
    // `/shell/12` where the build wrote `shell/12.html`.
    if let Some(last) = parts.last() {
        if !last.contains('.') {
            let html = candidate.with_file_name(format!("{last}.html"));
            if html.is_file() {
                return Resolved::File(html);
            }
        }
    }
    // SPA-safe: fall back to the nearest `index.html` at or above the requested path.
    let mut probe = parts.clone();
    loop {
        let dir = probe.iter().fold(root.to_path_buf(), |p, s| p.join(s));
        let index = dir.join("index.html");
        if index.is_file() {
            return Resolved::File(index);
        }
        if probe.pop().is_none() {
            break;
        }
    }
    Resolved::NotFound
}

/// `Content-Type` for a file, by extension.
pub fn content_type(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "txt" => "text/plain; charset=utf-8",
        "xml" => "application/xml; charset=utf-8",
        "wasm" => "application/wasm",
        "webmanifest" => "application/manifest+json",
        "mp3" => "audio/mpeg",
        "mp4" => "video/mp4",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    }
}

/// The page shown when `$BYO_HOME/site` has no build in it.
pub fn placeholder(site_dir: &Path) -> String {
    format!(
        r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>{title} — site not built</title>
<style>
  :root {{ color-scheme: light dark; }}
  body {{ font: 16px/1.6 ui-sans-serif, system-ui, sans-serif; margin: 0;
         display: grid; place-items: center; min-height: 100vh; background: #fdf7f2; color: #3b2f2a; }}
  main {{ max-width: 38rem; padding: 2rem; }}
  h1 {{ font-size: 1.6rem; margin: 0 0 .5rem; }}
  code, pre {{ font-family: ui-monospace, SFMono-Regular, Menlo, monospace; }}
  pre {{ background: #f2e6da; padding: .8rem 1rem; border-radius: .6rem; overflow-x: auto; }}
  a {{ color: #a2566f; }}
  @media (prefers-color-scheme: dark) {{
    body {{ background: #241d2b; color: #ede6f2; }}
    pre {{ background: #332a3c; }}
  }}
</style></head>
<body><main>
<h1>🌱 The site has not been built yet</h1>
<p>The API below is live, but there is no site in
   <code>{site_dir}</code>.</p>
<p>Build and install it with:</p>
<pre>byo site --rebuild</pre>
<p>or re-run <code>./install.sh</code> from the BuildYourOwn repo
   (it runs <code>npm ci &amp;&amp; npm run build</code> in <code>site/</code>).</p>
<p>Meanwhile the JSON API works: <a href="/api/health">/api/health</a>,
   <a href="/api/progress">/api/progress</a>, <a href="/api/runs">/api/runs</a>.</p>
</main></body></html>
"#,
        title = crate::branding::title(),
        site_dir = site_dir.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let w = |rel: &str| {
            let p = d.path().join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, "x").unwrap();
        };
        w("index.html");
        w("shell/index.html");
        w("shell/12/index.html");
        w("kafka/index.html");
        w("about.html");
        w("_app/immutable/app.js");
        w("favicon.svg");
        d
    }

    #[test]
    fn serves_root_and_directory_indexes() {
        let d = build();
        let r = d.path();
        assert_eq!(resolve(r, "/"), Resolved::File(r.join("index.html")));
        assert_eq!(
            resolve(r, "/shell/"),
            Resolved::File(r.join("shell/index.html"))
        );
        assert_eq!(
            resolve(r, "/shell/12/"),
            Resolved::File(r.join("shell/12/index.html"))
        );
    }

    #[test]
    fn redirects_directories_without_a_trailing_slash() {
        let d = build();
        assert_eq!(
            resolve(d.path(), "/shell"),
            Resolved::Redirect("/shell/".into())
        );
        assert_eq!(
            resolve(d.path(), "/shell/12"),
            Resolved::Redirect("/shell/12/".into())
        );
    }

    #[test]
    fn serves_files_and_assets() {
        let d = build();
        let r = d.path();
        assert_eq!(
            resolve(r, "/favicon.svg"),
            Resolved::File(r.join("favicon.svg"))
        );
        assert_eq!(
            resolve(r, "/_app/immutable/app.js"),
            Resolved::File(r.join("_app/immutable/app.js"))
        );
    }

    #[test]
    fn extensionless_pages_find_their_html() {
        let d = build();
        assert_eq!(
            resolve(d.path(), "/about"),
            Resolved::File(d.path().join("about.html"))
        );
    }

    #[test]
    fn unknown_paths_fall_back_to_the_nearest_index() {
        let d = build();
        let r = d.path();
        assert_eq!(
            resolve(r, "/shell/999"),
            Resolved::File(r.join("shell/index.html"))
        );
        assert_eq!(
            resolve(r, "/nope/deep/deeper"),
            Resolved::File(r.join("index.html"))
        );
    }

    #[test]
    fn nothing_escapes_the_root() {
        let d = build();
        assert_eq!(resolve(d.path(), "/../../etc/passwd"), Resolved::NotFound);
        assert_eq!(resolve(d.path(), "/shell/../../x"), Resolved::NotFound);
    }

    #[test]
    fn not_found_without_any_index() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(resolve(d.path(), "/anything"), Resolved::NotFound);
    }

    #[test]
    fn mime_types() {
        assert_eq!(
            content_type(Path::new("a/b.html")),
            "text/html; charset=utf-8"
        );
        assert_eq!(
            content_type(Path::new("a/b.JS")),
            "text/javascript; charset=utf-8"
        );
        assert_eq!(content_type(Path::new("a/b.woff2")), "font/woff2");
        assert_eq!(content_type(Path::new("a/b")), "application/octet-stream");
    }

    #[test]
    fn placeholder_tells_you_what_to_run() {
        let p = placeholder(Path::new("/data/site"));
        assert!(p.contains("byo site --rebuild"));
        assert!(p.contains("/data/site"));
    }
}
