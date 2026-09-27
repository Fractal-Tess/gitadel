//! Query-string construction for API paths.

use url::form_urlencoded;

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

#[cfg(test)]
mod tests {
    use super::*;

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
