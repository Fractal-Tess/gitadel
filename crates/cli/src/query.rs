//! Query-string construction for API paths.

use anyhow::{Result, bail};
use reqwest::Method;
use serde_json::Value;
use url::form_urlencoded;

use crate::ApiClient;

/// Appends the present `pairs` to `path` as an encoded query string.
pub(crate) fn with_query(path: String, pairs: &[(&str, Option<String>)]) -> String {
    let mut serializer = form_urlencoded::Serializer::new(String::new());
    let mut any = false;
    for (key, value) in pairs {
        if let Some(value) = value {
            serializer.append_pair(key, value);
            any = true;
        }
    }
    if any {
        format!("{path}?{}", serializer.finish())
    } else {
        path
    }
}

/// The most entries a paginated list endpoint returns per request.
const LIST_PAGE_SIZE: usize = 100;

/// `path` asking for one full-sized page of a paginated list.
fn page_path(path: &str, page: usize) -> String {
    let separator = if path.contains('?') { '&' } else { '?' };
    format!("{path}{separator}page={page}&per_page={LIST_PAGE_SIZE}")
}

/// Adds one fetched page to `items`, returning whether another page may
/// follow.
///
/// A server that predates pagination ignores `page` and answers every request
/// with the whole list, so a page that repeats the previous one ends the walk
/// instead of being appended forever.
fn append_page(items: &mut Vec<Value>, page: Vec<Value>) -> bool {
    let repeated = !page.is_empty() && items.ends_with(&page);
    let full = page.len() == LIST_PAGE_SIZE;
    if !repeated {
        items.extend(page);
    }
    full && !repeated
}

/// Fetches every page of a paginated list endpoint as one JSON array.
pub(crate) async fn all_pages(api: &ApiClient, path: &str) -> Result<Value> {
    let mut items = Vec::new();
    for page in 1.. {
        let response = api
            .request(Method::GET, &page_path(path, page), None)
            .await?;
        let Value::Array(entries) = response else {
            bail!("Gitadel returned a list that is not an array");
        };
        if !append_page(&mut items, entries) {
            break;
        }
    }
    Ok(Value::Array(items))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn page_path_extends_an_existing_query() {
        assert_eq!(page_path("x", 1), "x?page=1&per_page=100");
        assert_eq!(
            page_path("x?state=open", 3),
            "x?state=open&page=3&per_page=100"
        );
    }

    fn entries(range: std::ops::Range<usize>) -> Vec<Value> {
        range.map(|id| json!({ "id": id })).collect()
    }

    #[test]
    fn append_page_continues_only_after_a_full_page() {
        let mut items = Vec::new();
        assert!(append_page(&mut items, entries(0..100)));
        assert!(!append_page(&mut items, entries(100..130)));
        assert_eq!(items, entries(0..130));

        let mut items = Vec::new();
        assert!(!append_page(&mut items, Vec::new()));
        assert!(items.is_empty());
    }

    #[test]
    fn append_page_stops_when_an_unpaginated_server_repeats_itself() {
        let mut items = Vec::new();
        assert!(append_page(&mut items, entries(0..100)));
        assert!(!append_page(&mut items, entries(0..100)));
        assert_eq!(items, entries(0..100));
    }

    #[test]
    fn with_query_encodes_present_pairs_only() {
        let path = with_query(
            "repositories/a/b/issues".to_owned(),
            &[("state", Some("open".to_owned())), ("q", None)],
        );
        assert_eq!(path, "repositories/a/b/issues?state=open");
    }

    #[test]
    fn with_query_encodes_reserved_characters() {
        let path = with_query("x".to_owned(), &[("q", Some("a&b c#".to_owned()))]);
        assert_eq!(path, "x?q=a%26b+c%23");
    }

    #[test]
    fn with_query_leaves_path_without_pairs() {
        assert_eq!(with_query("x".to_owned(), &[("q", None)]), "x");
    }
}
