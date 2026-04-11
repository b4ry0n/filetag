use anyhow::{Result, bail};
use rusqlite::{Connection, params_from_iter};

// ---------------------------------------------------------------------------
// AST
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum Expr {
    Tag(String),                             // tag exists on file
    TagValue(String, CmpOp, String),         // tag <op> value
    Glob(String),                            // genre/* style wildcard
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Not(Box<Expr>),
}

#[derive(Debug, Clone, Copy)]
pub enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
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
        // Word / identifier (allows /, *, -, _ and alphanumeric, and .)
        let mut word = String::new();
        while let Some(&c) = chars.peek() {
            if c.is_alphanumeric() || c == '/' || c == '*' || c == '-' || c == '_' || c == '.' {
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
                format!(
                    "f.id IN (SELECT ft.file_id FROM file_tags ft JOIN tags t ON t.id = ft.tag_id WHERE t.name = {} AND ft.value {} {})",
                    pn, sql_op, pv
                )
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

/// Execute a query expression and return matching file paths.
pub fn execute(conn: &Connection, expr: &Expr) -> Result<Vec<String>> {
    let mut qb = QueryBuilder::new();
    let condition = qb.build_condition(expr);
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
pub fn execute_with_tags(
    conn: &Connection,
    expr: &Expr,
) -> Result<Vec<(String, Vec<(String, Option<String>)>)>> {
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
        assert!(matches!(expr, Expr::TagValue(ref t, CmpOp::Ge, ref v) if t == "year" && v == "2020"));
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
}
