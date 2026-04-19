mod ai;
mod api;
mod archive;
mod extract;
mod preview;
mod state;
mod types;
mod video;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use axum::{
    Router,
    http::{HeaderValue, header},
    routing::{get, post},
};
use clap::Parser;
use filetag_lib::db;
use rusqlite::Connection;
use tower_http::{limit::RequestBodyLimitLayer, set_header::SetResponseHeaderLayer};

use ai::AiProgress;
use filetag_lib::db::TagRoot;
use state::{AppState, resolve_names, terminal_width};

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

#[derive(Parser)]
#[command(name = "filetag-web", about = "Web interface for filetag", version)]
struct Args {
    /// Database root directory (default: current directory)
    path: Option<PathBuf>,

    /// Port to listen on
    #[arg(short, long, default_value_t = 3000)]
    port: u16,

    /// Address to bind to
    #[arg(short, long, default_value = "127.0.0.1")]
    bind: String,

    /// Do not automatically include ancestor databases (stop at the current root)
    #[arg(long)]
    no_parents: bool,
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let root = args.path.unwrap_or_else(|| ".".into());
    let root =
        std::fs::canonicalize(&root).with_context(|| format!("resolving {}", root.display()))?;

    // Open primary database and collect all explicitly linked databases.
    let (conn, root) = db::find_and_open(&root)?;
    let mut all_dbs = db::collect_all_databases(conn, root.clone(), !args.no_parents)?;

    // Discover nested databases by scanning the filesystem.
    {
        let mut visited: std::collections::HashSet<PathBuf> = all_dbs
            .iter()
            .filter_map(|db| std::fs::canonicalize(&db.root).ok())
            .collect();

        let scan_roots: Vec<PathBuf> = all_dbs.iter().map(|db| db.root.clone()).collect();

        use std::io::Write as _;
        let term_width = terminal_width();
        let mut on_dir = |dir: &std::path::Path| {
            let prefix = "Scanning ";
            let suffix = "...";
            let budget = term_width.saturating_sub(prefix.len() + suffix.len() + 1);
            let path_str = dir.display().to_string();
            let display = if path_str.len() > budget {
                let tail_bytes = budget.saturating_sub(1);
                let start = path_str.len().saturating_sub(tail_bytes);
                let start = path_str
                    .char_indices()
                    .map(|(i, _)| i)
                    .find(|&i| i >= start)
                    .unwrap_or(path_str.len());
                format!("…{}", &path_str[start..])
            } else {
                path_str
            };
            eprint!("\r\x1b[K{prefix}{display}{suffix}");
            let _ = std::io::stderr().flush();
        };

        for scan_root in &scan_roots {
            let found = db::scan_for_databases(scan_root, &mut visited, 10, &mut on_dir);
            all_dbs.extend(found);
        }

        eprint!("\r\x1b[K");
        let _ = std::io::stderr().flush();
    }

    // Sort so that ancestor (shorter path) databases come first.
    all_dbs.sort_by_key(|db| db.root.components().count());

    // Build named roots.
    let raw_names: Vec<String> = all_dbs
        .iter()
        .map(|db| {
            let conn_tmp = Connection::open(db.root.join(".filetag").join("db.sqlite3")).ok();
            conn_tmp
                .as_ref()
                .and_then(|c| db::get_setting(c, "name").ok().flatten())
                .unwrap_or_else(|| {
                    db.root
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| db.root.display().to_string())
                })
        })
        .collect();
    let names = resolve_names(raw_names);

    let roots: Vec<TagRoot> = all_dbs
        .into_iter()
        .zip(names)
        .map(|(open_db, name)| TagRoot {
            name,
            db_path: open_db.root.join(".filetag").join("db.sqlite3"),
            dev: db::volume_id(&open_db.root),
            entry_point: true,
            root: open_db.root,
        })
        .collect();

    // Mark entry points.
    let roots: Vec<TagRoot> = {
        let paths: Vec<PathBuf> = roots.iter().map(|r| r.root.clone()).collect();
        roots
            .into_iter()
            .map(|mut r| {
                let has_ancestor = paths
                    .iter()
                    .any(|other| other != &r.root && r.root.starts_with(other));
                r.entry_point = !has_ancestor;
                r
            })
            .collect()
    };

    if roots.is_empty() {
        anyhow::bail!("no databases found");
    }

    let state = Arc::new(AppState {
        roots,
        ai_progress: std::sync::Mutex::new(AiProgress::default()),
    });

    let app = Router::new()
        .route("/", get(api::index_html))
        .route("/css/base.css", get(api::css_base))
        .route("/css/layout.css", get(api::css_layout))
        .route("/css/toolbar.css", get(api::css_toolbar))
        .route("/css/cards.css", get(api::css_cards))
        .route("/css/detail.css", get(api::css_detail))
        .route("/css/viewer.css", get(api::css_viewer))
        .route("/js/utils.js", get(api::js_utils))
        .route("/js/state.js", get(api::js_state))
        .route("/js/tags.js", get(api::js_tags))
        .route("/js/render.js", get(api::js_render))
        .route("/js/detail.js", get(api::js_detail))
        .route("/js/actions.js", get(api::js_actions))
        .route("/js/lightbox.js", get(api::js_lightbox))
        .route("/js/viewer.js", get(api::js_viewer))
        .route("/js/main.js", get(api::js_main))
        .route("/favicon.svg", get(api::favicon))
        .route("/api/roots", get(api::api_roots))
        .route("/api/roots/reorder", post(api::api_reorder_roots))
        .route("/api/db/rename", post(api::api_rename_db))
        .route("/api/info", get(api::api_info))
        .route("/api/cache/clear", post(api::api_cache_clear))
        .route("/api/tags", get(api::api_tags))
        .route("/api/tag-values", get(api::api_tag_values))
        .route("/api/files", get(api::api_files))
        .route("/api/search", get(api::api_search))
        .route("/api/file", get(api::api_file_detail))
        .route("/api/tag", post(api::api_tag))
        .route("/api/untag", post(api::api_untag))
        .route("/api/tag-color", post(api::api_tag_color))
        .route("/api/rename-tag", post(api::api_rename_tag))
        .route("/api/delete-tag", post(api::api_delete_tag))
        .route("/api/synonym/add", post(api::api_add_synonym))
        .route("/api/synonym/remove", post(api::api_remove_synonym))
        .route("/api/zip/pages", get(archive::api_zip_pages))
        .route("/api/zip/page", get(archive::api_zip_page))
        .route("/api/zip/thumb", get(archive::api_zip_thumb))
        .route("/api/zip/entries", get(archive::api_zip_entries))
        .route("/api/dir/images", get(archive::api_dir_images))
        .route("/preview/{*path}", get(preview::preview_handler))
        .route("/thumb/{*path}", get(preview::thumb_handler))
        .route("/api/vthumbs", get(video::api_vthumbs))
        .route("/api/vthumbs/pregenerate", post(video::api_vthumbs_pregen))
        .route("/api/dir-thumbs", get(preview::api_dir_thumbs))
        .route("/api/ai/analyse", post(ai::api_ai_analyse))
        .route("/api/ai/analyse-batch", post(ai::api_ai_analyse_batch))
        .route("/api/ai/clear-tags", post(ai::api_ai_clear_tags))
        .route("/api/ai/status", get(ai::api_ai_status))
        .route("/api/ai/config", get(ai::api_ai_config_get))
        .route("/api/ai/config", post(ai::api_ai_config_set))
        .route("/api/settings", get(api::api_settings_get))
        .route("/api/settings", post(api::api_settings_set))
        .with_state(state.clone())
        // Deny framing and MIME-sniffing; restrict to same-origin requests only.
        .layer(SetResponseHeaderLayer::if_not_present(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::X_FRAME_OPTIONS,
            HeaderValue::from_static("DENY"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CONTENT_SECURITY_POLICY,
            // Tighten: allow same-origin for everything except images (data: for
            // base64 thumbs) and media (blob: for HLS).  No eval, no inline JS
            // (scripts are separate files), inline styles permitted for dynamic UI.
            HeaderValue::from_static(
                "default-src 'self'; \
                 script-src 'self'; \
                 style-src 'self' 'unsafe-inline'; \
                 img-src 'self' data: blob:; \
                 media-src 'self' blob:; \
                 connect-src 'self'; \
                 frame-ancestors 'none'",
            ),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::REFERRER_POLICY,
            HeaderValue::from_static("same-origin"),
        ))
        // Limit JSON/form request bodies to 32 MiB (prevents OOM from huge uploads).
        .layer(RequestBodyLimitLayer::new(32 * 1024 * 1024));

    let addr = format!("{}:{}", args.bind, args.port);

    // Warn when binding to a non-loopback address: the web UI has no
    // authentication and exposes local files.
    let is_loopback = matches!(args.bind.as_str(), "127.0.0.1" | "::1" | "localhost");
    if !is_loopback {
        eprintln!(
            "WARNING: filetag-web is bound to {} — the interface has no authentication. \
             Make sure access is restricted at the network level.",
            args.bind
        );
    }

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("binding to {}", addr))?;

    // Build parent index for the tree display.
    // Sort roots: shallower paths first (so parents always precede children),
    // then alphabetically by name within the same depth.
    let mut sorted_roots: Vec<_> = state.roots.iter().collect();
    sorted_roots.sort_by(|a, b| {
        let da = a.root.components().count();
        let db = b.root.components().count();
        da.cmp(&db)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    let n = sorted_roots.len();
    let mut parent_idx: Vec<Option<usize>> = vec![None; n];
    for (i, entry) in parent_idx.iter_mut().enumerate().skip(1) {
        let mut best: Option<usize> = None;
        let mut best_depth = 0usize;
        for j in 0..i {
            let comp = sorted_roots[j].root.components().count();
            if sorted_roots[i].root.starts_with(&sorted_roots[j].root) && comp > best_depth {
                best_depth = comp;
                best = Some(j);
            }
        }
        *entry = best;
    }
    let top_level_count = parent_idx.iter().filter(|p| p.is_none()).count();

    // Build children lists so we can print in DFS order.
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, parent) in parent_idx.iter().enumerate().take(n) {
        if let Some(p) = parent {
            children[*p].push(i);
        }
    }

    // Collect print order via iterative DFS from each top-level root.
    let mut print_order: Vec<usize> = Vec::with_capacity(n);
    let roots_iter: Vec<usize> = (0..n).filter(|&i| parent_idx[i].is_none()).collect();
    let mut stack: Vec<usize> = roots_iter.iter().rev().copied().collect();
    while let Some(i) = stack.pop() {
        print_order.push(i);
        for &c in children[i].iter().rev() {
            stack.push(c);
        }
    }

    println!("filetag-web at http://{}", addr);
    for &i in &print_order {
        // Rebuild the ancestor chain for i using parent_idx.
        let mut chain: Vec<usize> = Vec::new();
        let mut cur = i;
        while let Some(p) = parent_idx[cur] {
            chain.push(p);
            cur = p;
        }
        chain.reverse();
        let depth = chain.len();

        let mut prefix = String::new();
        let cont_end = depth.saturating_sub(1);
        for k in 0..cont_end {
            // Draw │ if the ancestor's sibling group (same parent) has more
            // entries after chain[k+1].
            let path_child = chain[k + 1];
            let parent_of_child = chain[k];
            let sibs = &children[parent_of_child];
            let child_is_last = sibs.last() == Some(&path_child);
            if child_is_last {
                prefix.push_str("   ");
            } else {
                prefix.push_str("│  ");
            }
        }

        let is_last = if let Some(p) = parent_idx[i] {
            children[p].last() == Some(&i)
        } else {
            roots_iter.last() == Some(&i)
        };
        let connector = if depth == 0 && top_level_count == 1 {
            ""
        } else if is_last {
            "└─ "
        } else {
            "├─ "
        };

        let label = format!(
            "{} ({})",
            sorted_roots[i].name,
            sorted_roots[i].root.display()
        );
        println!("  {}{}{}", prefix, connector, label);
    }
    axum::serve(listener, app).await?;

    Ok(())
}
