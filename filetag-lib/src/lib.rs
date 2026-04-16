pub mod db;
pub mod query;
pub mod registry;
#[cfg(unix)]
pub mod view;

/// A list of (tag_name, optional_value) pairs.
pub type TagList = Vec<(String, Option<String>)>;

/// Parse a single tag argument: `"year=2024"` → `("year", Some("2024"))`,
/// `"genre/rock"` → `("genre/rock", None)`.
pub fn parse_tag(s: &str) -> (String, Option<String>) {
    if let Some(eq) = s.find('=') {
        (s[..eq].to_string(), Some(s[eq + 1..].to_string()))
    } else {
        (s.to_string(), None)
    }
}
