//! Query parser and SQL code-generator for the filetag query language.
//!
//! # Grammar
//!
//! ```text
//! expr     = or_expr
//! or_expr  = and_expr ("or"  and_expr)*
//! and_expr = not_expr ("and" not_expr)*
//! not_expr = "not" not_expr | primary
//! primary  = "(" expr ")" | tag_value | tag_or_glob
//! tag_value = IDENT op VALUE
//! op       = "=" | "!=" | "<" | "<=" | ">" | ">=" | "eq" | "ne" | "lt" | "le" | "gt" | "ge"
//! ```
//!
//! Tag names may contain `/` (e.g. `genre/rock`). Glob patterns use `*`
//! (e.g. `genre/*`). Quoted strings (double-quoted) are supported for names
//! that contain spaces.

use anyhow::{Result, bail};
use rusqlite::{Connection, params_from_iter};

// ---------------------------------------------------------------------------
// AST
// ---------------------------------------------------------------------------

/// A node in the query abstract syntax tree.
#[derive(Debug, Clone)]
pub enum Expr {
    /// Tag must be present on the file (any value is accepted).
    Tag(String),
    /// Tag must have a value satisfying the given comparison, e.g. `year>=2020`.
    TagValue(String, CmpOp, String),
    /// Wildcard match against tag names, e.g. `genre/*`.
    Glob(String),
    /// Logical file-type filter, e.g. `type:image`.
    FileType(String),
    /// Both child expressions must match.
    And(Box<Expr>, Box<Expr>),
    /// At least one child expression must match.
    Or(Box<Expr>, Box<Expr>),
    /// The child expression must not match.
    Not(Box<Expr>),
}

/// Comparison operator used in [`Expr::TagValue`].
#[derive(Debug, Clone, Copy)]
pub enum CmpOp {
    /// Equal (`=` or `eq`).
    Eq,
    /// Not equal (`!=` or `ne`).
    Ne,
    /// Less than (`<` or `lt`).
    Lt,
    /// Less than or equal (`<=` or `le`).
    Le,
    /// Greater than (`>` or `gt`).
    Gt,
    /// Greater than or equal (`>=` or `ge`).
    Ge,
}

// ---------------------------------------------------------------------------
// Parser – recursive descent
//
// Grammar:
//   expr     = or_expr
//   or_expr  = and_expr ("or" and_expr)*
//   and_expr = not_expr ("and" not_expr)*
//   not_expr = "not" not_expr | primary
//   primary  = "(" expr ")" | tag_value | tag_or_glob
//   tag_value = IDENT op VALUE
//   op       = "=" | "!=" | "<" | "<=" | ">" | ">=" | "eq" | "ne" | "lt" | "le" | "gt" | "ge"
// ---------------------------------------------------------------------------

struct Parser {
    tokens: Vec<String>,
    pos: usize,
}

/// Parse a query string into an [`Expr`] AST.
///
/// Returns an error if the input is empty or syntactically invalid.
pub fn parse(input: &str) -> Result<Expr> {
    let tokens = tokenize(input)?;
    if tokens.is_empty() {
        bail!("empty query");
    }
    let mut parser = Parser { tokens, pos: 0 };
    let expr = parser.parse_or()?;
    if parser.pos < parser.tokens.len() {
        bail!(
            "unexpected token '{}' at position {}",
            parser.tokens[parser.pos],
            parser.pos
        );
    }
    Ok(expr)
}

fn tokenize(input: &str) -> Result<Vec<String>> {
    let mut tokens = Vec::new();
    let mut chars = input.chars().peekable();
    while let Some(&ch) = chars.peek() {
        if ch.is_whitespace() {
            chars.next();
            continue;
        }
        if ch == '(' || ch == ')' {
            tokens.push(ch.to_string());
            chars.next();
            continue;
        }
        // Quoted strings
        if ch == '"' || ch == '\'' {
            let quote = ch;
            chars.next();
            let mut word = String::new();
            loop {
                match chars.next() {
                    Some(c) if c == quote => break,
                    Some(c) => word.push(c),
                    None => bail!("unterminated quoted string"),
                }
            }
            if word.is_empty() {
                bail!("empty quoted string");
            }
            tokens.push(word);
            continue;
        }
        // Multi-char operators
        if ch == '!' {
            chars.next();
            if chars.peek() == Some(&'=') {
                chars.next();
                tokens.push("!=".into());
            } else {
                bail!("unexpected '!' without '='");
            }
            continue;
        }
        if ch == '<' {
            chars.next();
            if chars.peek() == Some(&'=') {
                chars.next();
                tokens.push("<=".into());
            } else {
                tokens.push("<".into());
            }
            continue;
        }
        if ch == '>' {
            chars.next();
            if chars.peek() == Some(&'=') {
                chars.next();
                tokens.push(">=".into());
            } else {
                tokens.push(">".into());
            }
            continue;
        }
        if ch == '=' {
            chars.next();
            tokens.push("=".into());
            continue;
        }
        // Word / identifier (allows /, *, -, _ and alphanumeric, . and :)
        let mut word = String::new();
        while let Some(&c) = chars.peek() {
            if c.is_alphanumeric()
                || c == '/'
                || c == '*'
                || c == '-'
                || c == '_'
                || c == '.'
                || c == ':'
            {
                word.push(c);
                chars.next();
            } else {
                break;
            }
        }
        if word.is_empty() {
            bail!("unexpected character '{}'", ch);
        }
        tokens.push(word);
    }
    Ok(tokens)
}

impl Parser {
    fn peek(&self) -> Option<&str> {
        self.tokens.get(self.pos).map(|s| s.as_str())
    }

    fn advance(&mut self) -> &str {
        let t = &self.tokens[self.pos];
        self.pos += 1;
        t
    }

    fn parse_or(&mut self) -> Result<Expr> {
        let mut left = self.parse_and()?;
        while self.peek() == Some("or") {
            self.advance();
            let right = self.parse_and()?;
            left = Expr::Or(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Expr> {
        let mut left = self.parse_not()?;
        while self.peek() == Some("and") {
            self.advance();
            let right = self.parse_not()?;
            left = Expr::And(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_not(&mut self) -> Result<Expr> {
        if self.peek() == Some("not") {
            self.advance();
            let expr = self.parse_not()?;
            return Ok(Expr::Not(Box::new(expr)));
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<Expr> {
        if self.peek() == Some("(") {
            self.advance();
            let expr = self.parse_or()?;
            if self.peek() != Some(")") {
                bail!("expected ')'");
            }
            self.advance();
            return Ok(expr);
        }

        let token = self
            .peek()
            .ok_or_else(|| anyhow::anyhow!("unexpected end of query"))?
            .to_string();
        self.advance();

        // type:xxx filter — must be checked before try-parse as comparison
        if let Some(kind) = token.strip_prefix("type:") {
            let kind = kind.to_ascii_lowercase();
            if kind.is_empty() {
                anyhow::bail!("expected a type name after 'type:'");
            }
            // Resolve aliases so the canonical name is stored in the AST.
            let canonical = match kind.as_str() {
                "img" | "photo" | "picture" | "pic" => "image".to_string(),
                "vid" | "movie" | "film" => "video".to_string(),
                "aud" | "music" | "sound" => "audio".to_string(),
                "doc" | "document" => "document".to_string(),
                "arc" | "archive" | "compressed" => "archive".to_string(),
                "txt" => "text".to_string(),
                "font" => "font".to_string(),
                other => other.to_string(),
            };
            return Ok(Expr::FileType(canonical));
        }

        // Check for comparison operator
        if let Some(op) = self.peek().and_then(parse_cmp_op) {
            self.advance();
            let value = self
                .peek()
                .ok_or_else(|| anyhow::anyhow!("expected value after operator"))?
                .to_string();
            self.advance();
            return Ok(Expr::TagValue(token, op, value));
        }

        // Glob?
        if token.contains('*') {
            return Ok(Expr::Glob(token));
        }

        Ok(Expr::Tag(token))
    }
}

fn parse_cmp_op(s: &str) -> Option<CmpOp> {
    match s {
        "=" | "eq" => Some(CmpOp::Eq),
        "!=" | "ne" => Some(CmpOp::Ne),
        "<" | "lt" => Some(CmpOp::Lt),
        "<=" | "le" => Some(CmpOp::Le),
        ">" | "gt" => Some(CmpOp::Gt),
        ">=" | "ge" => Some(CmpOp::Ge),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// SQL generation
// ---------------------------------------------------------------------------

struct QueryBuilder {
    /// Bind parameters collected during building.
    bind_params: Vec<String>,
}

impl QueryBuilder {
    fn new() -> Self {
        Self {
            bind_params: Vec::new(),
        }
    }

    fn param(&mut self, value: &str) -> String {
        self.bind_params.push(value.to_string());
        format!("?{}", self.bind_params.len())
    }

    /// Generate a SQL condition that selects `files.id` matching `expr`.
    fn build_condition(&mut self, expr: &Expr) -> String {
        match expr {
            Expr::Tag(name) => {
                let p = self.param(name);
                format!(
                    "f.id IN (SELECT ft.file_id FROM file_tags ft JOIN tags t ON t.id = ft.tag_id WHERE t.name = {})",
                    p
                )
            }
            Expr::Glob(pattern) => {
                let like_pattern = pattern.replace('*', "%");
                let p = self.param(&like_pattern);
                format!(
                    "f.id IN (SELECT ft.file_id FROM file_tags ft JOIN tags t ON t.id = ft.tag_id WHERE t.name LIKE {})",
                    p
                )
            }
            Expr::TagValue(name, op, value) => {
                let pn = self.param(name);
                let pv = self.param(value);
                let sql_op = match op {
                    CmpOp::Eq => "=",
                    CmpOp::Ne => "!=",
                    CmpOp::Lt => "<",
                    CmpOp::Le => "<=",
                    CmpOp::Gt => ">",
                    CmpOp::Ge => ">=",
                };
                // Use numeric comparison when the query value is a number
                let value_expr = if value.parse::<f64>().is_ok() {
                    format!("CAST(ft.value AS REAL) {} CAST({} AS REAL)", sql_op, pv)
                } else {
                    format!("ft.value {} {}", sql_op, pv)
                };
                format!(
                    "f.id IN (SELECT ft.file_id FROM file_tags ft JOIN tags t ON t.id = ft.tag_id WHERE t.name = {} AND {})",
                    pn, value_expr
                )
            }
            Expr::FileType(kind) => {
                // Build a path LIKE condition for all known extensions of this type.
                let exts = file_type_extensions(kind);
                if exts.is_empty() {
                    // Unknown type — match nothing.
                    return "1=0".to_string();
                }
                let conditions: Vec<String> = exts
                    .iter()
                    .flat_map(|ext| {
                        // Match both lower and upper case suffixes to be safe.
                        let lower = format!("%.{}", ext.to_lowercase());
                        let upper = format!("%.{}", ext.to_uppercase());
                        let pl = self.param(&lower);
                        let pu = self.param(&upper);
                        [format!("f.path LIKE {}", pl), format!("f.path LIKE {}", pu)]
                    })
                    .collect();
                format!("({})", conditions.join(" OR "))
            }
            Expr::And(a, b) => {
                let ca = self.build_condition(a);
                let cb = self.build_condition(b);
                format!("({} AND {})", ca, cb)
            }
            Expr::Or(a, b) => {
                let ca = self.build_condition(a);
                let cb = self.build_condition(b);
                format!("({} OR {})", ca, cb)
            }
            Expr::Not(inner) => {
                let ci = self.build_condition(inner);
                format!("NOT {}", ci)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Alias resolution
// ---------------------------------------------------------------------------

/// Walk `expr` and replace any tag name (or key name in a `TagValue`) that is
/// registered as a synonym with its canonical tag name.  Glob patterns are
/// left unchanged because they may expand to multiple tags.
fn resolve_aliases(conn: &Connection, expr: Expr) -> Result<Expr> {
    Ok(match expr {
        Expr::Tag(name) => Expr::Tag(canonical_name(conn, &name)?),
        Expr::TagValue(name, op, val) => Expr::TagValue(canonical_name(conn, &name)?, op, val),
        Expr::Glob(p) => Expr::Glob(p),
        Expr::FileType(k) => Expr::FileType(k),
        Expr::And(a, b) => Expr::And(
            Box::new(resolve_aliases(conn, *a)?),
            Box::new(resolve_aliases(conn, *b)?),
        ),
        Expr::Or(a, b) => Expr::Or(
            Box::new(resolve_aliases(conn, *a)?),
            Box::new(resolve_aliases(conn, *b)?),
        ),
        Expr::Not(inner) => Expr::Not(Box::new(resolve_aliases(conn, *inner)?)),
    })
}

/// Look up the canonical tag name for `name`.  Returns `name` unchanged when
/// it is not registered as a synonym.
fn canonical_name(conn: &Connection, name: &str) -> Result<String> {
    use rusqlite::params;
    let canonical: Option<String> = conn
        .prepare_cached(
            "SELECT t.name FROM tag_synonyms ts \
             JOIN tags t ON t.id = ts.canonical_id \
             WHERE ts.alias = ?1",
        )?
        .query_row(params![name], |r| r.get(0))
        .ok();
    Ok(canonical.unwrap_or_else(|| name.to_string()))
}

/// Execute a query expression and return matching file paths.
///
/// Tag names and key names in the expression are resolved through the synonym
/// table before the SQL is generated, so searching for an alias produces the
/// same results as searching for the canonical tag name.
pub fn execute(conn: &Connection, expr: &Expr) -> Result<Vec<String>> {
    let resolved = resolve_aliases(conn, expr.clone())?;
    let mut qb = QueryBuilder::new();
    let condition = qb.build_condition(&resolved);
    let sql = format!(
        "SELECT f.path FROM files f WHERE {} ORDER BY f.path",
        condition
    );

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(qb.bind_params.iter()), |row| {
        row.get::<_, String>(0)
    })?;

    let mut paths = Vec::new();
    for row in rows {
        paths.push(row?);
    }
    Ok(paths)
}

/// Execute a query and return paths with their tags.
pub fn execute_with_tags(conn: &Connection, expr: &Expr) -> Result<Vec<(String, crate::TagList)>> {
    let paths = execute(conn, expr)?;
    let mut result = Vec::new();
    for path in paths {
        if let Some(rec) = crate::db::file_by_path(conn, &path)? {
            let tags = crate::db::tags_for_file(conn, rec.id)?;
            result.push((path, tags));
        }
    }
    Ok(result)
}

// ---------------------------------------------------------------------------
// File type → extension mapping
// ---------------------------------------------------------------------------

/// Return the list of file extensions (lowercase, without leading dot) that
/// belong to the given logical file type name.  The caller folds both cases.
fn file_type_extensions(kind: &str) -> &'static [&'static str] {
    match kind {
        "image" => &[
            "jpg", "jpeg", "png", "gif", "webp", "bmp", "tiff", "tif", "avif", "heic", "heif",
            "ico", "svg", "psd", "xcf", "arw", "cr2", "cr3", "nef", "orf", "rw2", "dng", "raf",
            "pef", "srw", "raw", "3fr", "x3f", "rwl", "iiq", "mef", "mos",
        ],
        "video" => &[
            "mp4", "mkv", "avi", "mov", "wmv", "flv", "webm", "m4v", "3gp", "f4v", "mpg", "mpeg",
            "m2v", "m2ts", "mts", "mxf", "rm", "rmvb", "divx", "vob", "ogv", "ogg", "dv", "asf",
            "amv", "mpe", "m1v", "mpv", "qt",
        ],
        "audio" => &[
            "mp3", "flac", "aac", "ogg", "opus", "m4a", "wav", "aiff", "aif", "wma", "alac", "ape",
            "mka", "wv", "tta", "dsf", "dff", "spx", "caf", "au",
        ],
        "document" => &[
            "pdf", "doc", "docx", "odt", "rtf", "xls", "xlsx", "ods", "ppt", "pptx", "odp",
            "pages", "numbers", "key", "epub", "mobi", "djvu", "tex", "md", "rst",
        ],
        "archive" => &[
            "zip", "tar", "gz", "bz2", "xz", "zst", "7z", "rar", "cbz", "cbr", "cb7", "cbt", "tgz",
            "tbz2", "txz", "iso", "dmg", "pkg", "deb", "rpm", "apk",
        ],
        "text" => &[
            "txt", "log", "csv", "tsv", "nfo", "ini", "cfg", "conf", "toml", "yaml", "yml", "json",
            "xml", "html", "htm", "css", "js", "ts", "rs", "py", "rb", "sh", "bash", "zsh", "fish",
            "c", "h", "cpp", "hpp", "java", "go", "swift", "kt", "sql", "lua", "pl", "r",
        ],
        "font" => &["ttf", "otf", "woff", "woff2", "eot", "fon"],
        _ => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_tag() {
        let expr = parse("music").unwrap();
        assert!(matches!(expr, Expr::Tag(ref s) if s == "music"));
    }

    #[test]
    fn parse_and_or() {
        let expr = parse("a and b or c").unwrap();
        // Should parse as (a AND b) OR c
        assert!(matches!(expr, Expr::Or(_, _)));
    }

    #[test]
    fn parse_not() {
        let expr = parse("not live").unwrap();
        assert!(matches!(expr, Expr::Not(_)));
    }

    #[test]
    fn parse_glob() {
        let expr = parse("genre/*").unwrap();
        assert!(matches!(expr, Expr::Glob(ref s) if s == "genre/*"));
    }

    #[test]
    fn parse_tag_value() {
        let expr = parse("year >= 2020").unwrap();
        assert!(
            matches!(expr, Expr::TagValue(ref t, CmpOp::Ge, ref v) if t == "year" && v == "2020")
        );
    }

    #[test]
    fn parse_complex() {
        let expr = parse("genre/rock and not live and (year >= 2020 or favorite)").unwrap();
        assert!(matches!(expr, Expr::And(_, _)));
    }

    #[test]
    fn parse_parens() {
        let expr = parse("(a or b) and c").unwrap();
        assert!(matches!(expr, Expr::And(_, _)));
    }

    #[test]
    fn parse_quoted_tag() {
        let expr = parse("\"Extra models\"").unwrap();
        assert!(matches!(expr, Expr::Tag(ref s) if s == "Extra models"));
    }

    #[test]
    fn parse_quoted_and() {
        let expr = parse("\"Extra models\" and 3D").unwrap();
        assert!(matches!(expr, Expr::And(_, _)));
    }
}
