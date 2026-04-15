mod ai;
mod api;
mod archive;
mod preview;
mod state;
mod types;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use axum::{
    Router,
    routing::{get, post},
};
use clap::Parser;
use filetag_lib::db;
use rusqlite::Connection;

use ai::AiProgress;
use state::{AppState, DbRoot, resolve_names, terminal_width};

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

    let roots: Vec<DbRoot> = all_dbs
        .into_iter()
        .zip(names)
        .map(|(open_db, name)| {
            #[cfg(unix)]
            let dev = {
                use std::os::unix::fs::MetadataExt;
                std::fs::metadata(&open_db.root).ok().map(|m| m.dev())
            };
            DbRoot {
                name,
                db_path: open_db.root.join(".filetag").join("db.sqlite3"),
                #[cfg(unix)]
                dev,
                entry_point: true,
                root: open_db.root,
            }
        })
        .collect();

    // Mark entry points.
    let roots: Vec<DbRoot> = {
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
        .route("/style.css", get(api::style_css))
        .route("/app.js", get(api::app_js))
        .route("/favicon.svg", get(api::favicon))
        .route("/api/roots", get(api::api_roots))
        .route("/api/roots/reorder", post(api::api_reorder_roots))
        .route("/api/db/rename", post(api::api_rename_db))
        .route("/api/info", get(api::api_info))
        .route("/api/cache/clear", post(api::api_cache_clear))
        .route("/api/tags", get(api::api_tags))
        .route("/api/files", get(api::api_files))
        .route("/api/search", get(api::api_search))
        .route("/api/file", get(api::api_file_detail))
        .route("/api/tag", post(api::api_tag))
        .route("/api/untag", post(api::api_untag))
        .route("/api/tag-color", post(api::api_tag_color))
        .route("/api/delete-tag", post(api::api_delete_tag))
        .route("/api/zip/pages", get(archive::api_zip_pages))
        .route("/api/zip/page", get(archive::api_zip_page))
        .route("/api/zip/thumb", get(archive::api_zip_thumb))
        .route("/api/zip/entries", get(archive::api_zip_entries))
        .route("/api/dir/images", get(archive::api_dir_images))
        .route("/hls.min.js", get(api::hls_js))
        .route("/hls/*path", get(preview::hls_handler))
        .route("/preview/*path", get(preview::preview_handler))
        .route("/thumb/*path", get(preview::thumb_handler))
        .route("/api/vthumbs", get(preview::api_vthumbs))
        .route(
            "/api/vthumbs/pregenerate",
            post(preview::api_vthumbs_pregen),
        )
        .route("/api/ai/analyse", post(ai::api_ai_analyse))
        .route("/api/ai/analyse-batch", post(ai::api_ai_analyse_batch))
        .route("/api/ai/clear-tags", post(ai::api_ai_clear_tags))
        .route("/api/ai/status", get(ai::api_ai_status))
        .route("/api/ai/config", get(ai::api_ai_config_get))
        .route("/api/ai/config", post(ai::api_ai_config_set))
        .with_state(state.clone());

    let addr = format!("{}:{}", args.bind, args.port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("binding to {}", addr))?;

    // Build parent index for the tree display.
    let n = state.roots.len();
    let mut parent_idx: Vec<Option<usize>> = vec![None; n];
    for (i, entry) in parent_idx.iter_mut().enumerate().skip(1) {
        let mut best: Option<usize> = None;
        let mut best_depth = 0usize;
        for j in 0..i {
            let comp = state.roots[j].root.components().count();
            if state.roots[i].root.starts_with(&state.roots[j].root) && comp > best_depth {
                best_depth = comp;
                best = Some(j);
            }
        }
        *entry = best;
    }
    let top_level_count = parent_idx.iter().filter(|p| p.is_none()).count();

    println!("filetag-web at http://{}", addr);
    for i in 0..n {
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
        for &anc in &chain[..cont_end] {
            let anc_is_last = (anc + 1..n).all(|j| parent_idx[j] != parent_idx[anc]);
            if anc_is_last {
                prefix.push_str("   ");
            } else {
                prefix.push_str("│  ");
            }
        }

        let is_last = (i + 1..n).all(|j| parent_idx[j] != parent_idx[i]);
        let connector = if depth == 0 && top_level_count == 1 {
            ""
        } else if is_last {
            "└─ "
        } else {
            "├─ "
        };

        let label = format!(
            "{} ({})",
            state.roots[i].name,
            state.roots[i].root.display()
        );
        println!("  {}{}{}", prefix, connector, label);
    }
    axum::serve(listener, app).await?;

    Ok(())
}
