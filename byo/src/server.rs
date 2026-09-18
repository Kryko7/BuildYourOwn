//! `byo site` — one HTTP port serving the built site and the JSON API.

use crate::api;
use crate::db;
use crate::paths::Paths;
use crate::static_files::{self, Resolved};
use anyhow::{anyhow, bail, Context, Result};
use rusqlite::Connection;
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tiny_http::{Header, Response, Server};

/// Ports we never take, even while hunting for a free one.
const RESERVED: &[u16] = &[4173, 9092];

/// Find a free port starting at `start`, skipping reserved ones.
pub fn pick_port(start: u16, tries: u16) -> Result<u16> {
    let mut port = start;
    for _ in 0..tries {
        if !RESERVED.contains(&port) && is_free(port) {
            return Ok(port);
        }
        port = port
            .checked_add(1)
            .ok_or_else(|| anyhow!("ran out of ports"))?;
    }
    bail!(
        "no free port in {start}..{} — pass --port",
        start.saturating_add(tries)
    )
}

fn is_free(port: u16) -> bool {
    TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, port))).is_ok()
}

/// Serve the site and the API until the process is interrupted.
pub fn serve(paths: &Paths, requested: Option<u16>, no_open: bool) -> Result<()> {
    let default = requested.unwrap_or(4321);
    let port = match requested {
        Some(p) if !is_free(p) => {
            let alt = pick_port(p.saturating_add(1), 50)?;
            eprintln!("port {p} is busy — using {alt} instead");
            alt
        }
        Some(p) => p,
        None => {
            let p = pick_port(default, 50)?;
            if p != default {
                eprintln!("port {default} is busy — using {p} instead");
            }
            p
        }
    };

    let site = paths.site();
    let conn = db::open(&paths.db())?;
    let ctx = Arc::new(api::Ctx {
        paths: paths.clone(),
        version: env!("CARGO_PKG_VERSION").into(),
    });
    let conn = Arc::new(Mutex::new(conn));

    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let server = Server::http(addr).map_err(|e| anyhow!("cannot listen on {addr}: {e}"))?;
    let server = Arc::new(server);
    let url = format!("http://127.0.0.1:{port}/");

    if site.is_dir() {
        println!("byo site: serving {} at {url}", site.display());
    } else {
        println!(
            "byo site: {} has no build — serving a placeholder at {url}",
            site.display()
        );
        println!("          run `byo site --rebuild` to build and install the site");
    }
    println!("byo site: API at {url}api/health   (Ctrl-C to stop)");

    if !no_open {
        if let Err(e) = open::that_detached(&url) {
            eprintln!("could not open a browser ({e}); open {url} yourself");
        }
    }

    let workers = 4;
    let mut handles = Vec::new();
    for _ in 0..workers {
        let server = Arc::clone(&server);
        let conn = Arc::clone(&conn);
        let ctx = Arc::clone(&ctx);
        let site = site.clone();
        handles.push(std::thread::spawn(move || loop {
            let Ok(request) = server.recv() else { return };
            if let Err(e) = respond(request, &conn, &ctx, &site) {
                eprintln!("byo site: {e:#}");
            }
        }));
    }
    for h in handles {
        let _ = h.join();
    }
    Ok(())
}

fn respond(
    mut request: tiny_http::Request,
    conn: &Mutex<Connection>,
    ctx: &api::Ctx,
    site: &Path,
) -> Result<()> {
    let method = request.method().as_str().to_string();
    let target = request.url().to_string();
    let mut body = String::new();
    if matches!(method.as_str(), "POST" | "PUT" | "PATCH") {
        request.as_reader().read_to_string(&mut body).ok();
    }
    let req = api::Request::new(&method, &target, &body);

    let api_response = {
        let guard = conn
            .lock()
            .map_err(|_| anyhow!("the database lock was poisoned"))?;
        api::handle(&guard, ctx, &req)
    };
    if let Some(r) = api_response {
        let response = Response::from_string(r.body)
            .with_status_code(r.status)
            .with_header(header("Content-Type", r.content_type))
            .with_header(header("Cache-Control", "no-store"));
        return request
            .respond(response)
            .context("cannot send the API response");
    }

    if !site.is_dir() {
        let response = Response::from_string(static_files::placeholder(site))
            .with_status_code(200)
            .with_header(header("Content-Type", "text/html; charset=utf-8"));
        return request
            .respond(response)
            .context("cannot send the placeholder page");
    }

    match static_files::resolve(site, &req.path) {
        Resolved::File(path) => {
            let data =
                std::fs::read(&path).with_context(|| format!("cannot read {}", path.display()))?;
            let cache = if req.path.starts_with("/_app/immutable/") {
                "public, max-age=31536000, immutable"
            } else {
                "no-cache"
            };
            let response = Response::from_data(data)
                .with_header(header("Content-Type", static_files::content_type(&path)))
                .with_header(header("Cache-Control", cache));
            request.respond(response).context("cannot send the file")
        }
        Resolved::Redirect(to) => {
            let response = Response::from_string("")
                .with_status_code(301)
                .with_header(header("Location", &to));
            request
                .respond(response)
                .context("cannot send the redirect")
        }
        Resolved::NotFound => {
            let response = Response::from_string(format!("404 — no such page: {}\n", req.path))
                .with_status_code(404)
                .with_header(header("Content-Type", "text/plain; charset=utf-8"));
            request.respond(response).context("cannot send the 404")
        }
    }
}

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).unwrap_or_else(|_| {
        Header::from_bytes(&b"X-Byo"[..], &b"header"[..]).expect("static header")
    })
}

/// `byo site --rebuild`: run `npm run build` in the repo's `site/` and install the result.
pub fn rebuild(paths: &Paths) -> Result<()> {
    let conn = db::open(&paths.db())?;
    let source = db::get_meta(&conn, "site_source")?
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
        .ok_or_else(|| {
            anyhow!(
                "I do not know where the site sources are.\n\
                 `install.sh` records them in the database as `site_source`; re-run ./install.sh \
                 from the BuildYourOwn repo, or set it with:\n\
                 \x20   byo db set-site-source /path/to/BuildYourOwn/site"
            )
        })?;
    println!("byo site: building {} …", source.display());
    let status = std::process::Command::new("npm")
        .arg("run")
        .arg("build")
        .current_dir(&source)
        .status()
        .with_context(|| format!("cannot run `npm run build` in {}", source.display()))?;
    if !status.success() {
        bail!("`npm run build` failed in {} ({status})", source.display());
    }
    let built = source.join("build");
    if !built.is_dir() {
        bail!("`npm run build` did not produce {}", built.display());
    }
    let dest = paths.site();
    if dest.exists() {
        std::fs::remove_dir_all(&dest)
            .with_context(|| format!("cannot clear {}", dest.display()))?;
    }
    copy_dir(&built, &dest)?;
    println!(
        "byo site: installed {} → {}",
        built.display(),
        dest.display()
    );
    Ok(())
}

/// Recursively copy a directory.
pub fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to).with_context(|| format!("cannot create {}", to.display()))?;
    for entry in
        std::fs::read_dir(from).with_context(|| format!("cannot read {}", from.display()))?
    {
        let entry = entry?;
        let src = entry.path();
        let dst = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&src, &dst)?;
        } else {
            std::fs::copy(&src, &dst)
                .with_context(|| format!("cannot copy {} → {}", src.display(), dst.display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_port_skips_busy_and_reserved() {
        let taken = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).unwrap();
        let busy = taken.local_addr().unwrap().port();
        let chosen = pick_port(busy, 20).unwrap();
        assert_ne!(chosen, busy);
        assert!(chosen > busy);
        assert!(!RESERVED.contains(&chosen));
    }

    #[test]
    fn copy_dir_copies_nested_trees() {
        let from = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(from.path().join("a/b")).unwrap();
        std::fs::write(from.path().join("a/b/c.txt"), "hi").unwrap();
        std::fs::write(from.path().join("top.html"), "<p>").unwrap();
        let to = tempfile::tempdir().unwrap();
        let dest = to.path().join("site");
        copy_dir(from.path(), &dest).unwrap();
        assert_eq!(
            std::fs::read_to_string(dest.join("a/b/c.txt")).unwrap(),
            "hi"
        );
        assert!(dest.join("top.html").is_file());
    }
}
