pub mod db;
pub mod query;
pub mod registry;
pub mod view;

/// A list of (tag_name, optional_value) pairs.
pub type TagList = Vec<(String, Option<String>)>;
