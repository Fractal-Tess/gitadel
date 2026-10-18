use std::collections::BTreeSet;

use super::{Body, Method, document, operations};

/// The routers nested under `/api/v1`, and the text that bounds each one.
const ROUTERS: &[(&str, &str, &str, &str)] = &[
    (
        "src/server.rs",
        include_str!("../server.rs"),
        "let api_router = Router::new()",
        "let app = Router",
    ),
    (
        "src/identity/mod.rs",
        include_str!("../identity/mod.rs"),
        "pub fn router() -> Router<IdentityState>",
        "\npub fn oauth_router",
    ),
    (
        "src/identity/mirror_identities.rs",
        include_str!("../identity/mirror_identities.rs"),
        "pub(super) fn router()",
        "\n}\n",
    ),
    (
        "src/repository/mod.rs",
        include_str!("../repository/mod.rs"),
        "pub fn router() -> Router<RepositoryState>",
        "\n}\n",
    ),
    (
        "src/actions/api.rs",
        include_str!("../actions/api.rs"),
        "pub(crate) fn router()",
        "\n}\n",
    ),
    (
        "src/actions/secrets.rs",
        include_str!("../actions/secrets.rs"),
        "pub(crate) fn router()",
        "\n}\n",
    ),
    (
        "src/actions/dispatch.rs",
        include_str!("../actions/dispatch.rs"),
        "pub(crate) fn router()",
        "\n}\n",
    ),
];

fn method(name: &str) -> Option<Method> {
    Some(match name {
        "get" => Method::Get,
        "post" => Method::Post,
        "put" => Method::Put,
        "patch" => Method::Patch,
        "delete" => Method::Delete,
        _ => return None,
    })
}

/// Every `(method, path)` the routers register, read from their source.
fn registered() -> BTreeSet<(String, Method)> {
    let mut found = BTreeSet::new();
    for (file, source, start, end) in ROUTERS {
        let begin = source
            .find(start)
            .unwrap_or_else(|| panic!("{file}: router start {start:?} not found"));
        let rest = &source[begin..];
        let body = &rest[..rest[start.len()..]
            .find(end)
            .map_or(rest.len(), |offset| offset + start.len())];
        let mut cursor = body;
        while let Some(index) = cursor.find(".route(") {
            cursor = &cursor[index + ".route(".len()..];
            let quoted = cursor.trim_start();
            let Some(after_quote) = quoted.strip_prefix('"') else {
                continue;
            };
            let path_end = after_quote.find('"').expect("route path is terminated");
            let path = &after_quote[..path_end];
            // The handler expression runs to the parenthesis closing `.route(`.
            let mut depth = 1;
            let mut handler_end = 0;
            for (offset, character) in after_quote[path_end..].char_indices() {
                match character {
                    '(' => depth += 1,
                    ')' => depth -= 1,
                    _ => {}
                }
                if depth == 0 {
                    handler_end = path_end + offset;
                    break;
                }
            }
            let handler = &after_quote[path_end..handler_end];
            for (index, _) in handler.match_indices('(') {
                let before = &handler[..index];
                let name_start = before
                    .rfind(|character: char| !character.is_ascii_alphanumeric() && character != '_')
                    .map_or(0, |position| position + 1);
                if let Some(method) = method(&before[name_start..]) {
                    found.insert((path.to_owned(), method));
                }
            }
        }
    }
    found
}

#[test]
fn every_api_route_is_described_and_nothing_else() {
    let registered = registered();
    assert!(
        registered.len() > 200,
        "parsed only {} routes",
        registered.len()
    );
    let described: BTreeSet<(String, Method)> = operations()
        .map(|operation| (operation.path.to_owned(), operation.method))
        .collect();
    let missing: Vec<_> = registered.difference(&described).collect();
    let stale: Vec<_> = described.difference(&registered).collect();
    assert!(
        missing.is_empty() && stale.is_empty(),
        "update src/openapi/operations\n  routes without a description: {missing:#?}\n  descriptions without a route: {stale:#?}"
    );
    assert_eq!(
        operations().count(),
        described.len(),
        "an operation is described twice"
    );
}

#[test]
fn the_document_is_valid_json_with_every_operation() {
    let document = serde_json::to_value(document()).unwrap();
    assert_eq!(
        document["openapi"].as_str().map(|version| &version[..2]),
        Some("3.")
    );
    let paths = document["paths"].as_object().unwrap();
    let count: usize = paths
        .values()
        .map(|item| item.as_object().unwrap().len())
        .sum();
    assert_eq!(count, operations().count());
    // Every schema reference resolves to a registered component.
    let schemas = document["components"]["schemas"].as_object().unwrap();
    let text = document.to_string();
    for reference in text.split("\"#/components/schemas/").skip(1) {
        let name = &reference[..reference.find('"').unwrap()];
        assert!(schemas.contains_key(name), "unresolved schema {name}");
    }
}

#[test]
fn operation_summaries_read_as_sentences() {
    for operation in operations() {
        assert!(
            operation
                .summary
                .chars()
                .next()
                .is_some_and(char::is_uppercase),
            "{} {:?}: summary should start with a capital",
            operation.path,
            operation.method
        );
        assert!(
            !operation.summary.ends_with('.'),
            "{}: no trailing period",
            operation.path
        );
        if matches!(operation.method, Method::Get) {
            assert!(
                !matches!(operation.request, Body::Json(_) | Body::Unspecified),
                "{}: GET requests have no body",
                operation.path
            );
        }
    }
}

/// Names whose schemas differ: two Rust types claiming one schema name,
/// which would silently overwrite each other in the document.
fn clashing_names(schemas: super::Schemas) -> Vec<String> {
    let mut seen = std::collections::BTreeMap::new();
    let mut clashes = Vec::new();
    for (name, schema) in schemas {
        let rendered = serde_json::to_string(&schema).unwrap();
        if let Some(previous) = seen.insert(name.clone(), rendered.clone())
            && previous != rendered
        {
            clashes.push(name);
        }
    }
    clashes
}

#[test]
fn distinct_types_never_share_a_schema_name() {
    let mut schemas = super::Schemas::new();
    for operation in operations() {
        for body in [operation.request, operation.response] {
            if let Body::Json(schema) = body {
                schema(&mut schemas);
            }
        }
    }
    assert_eq!(
        clashing_names(schemas),
        Vec::<String>::new(),
        "give one of each pair `#[schema(as = …)]`"
    );
}

#[test]
fn clashing_schema_names_are_detected() {
    mod first {
        #[derive(utoipa::ToSchema)]
        #[expect(dead_code, reason = "only its schema is used")]
        pub struct Same {
            pub a: String,
        }
    }
    mod second {
        #[derive(utoipa::ToSchema)]
        #[expect(dead_code, reason = "only its schema is used")]
        pub struct Same {
            pub b: u32,
        }
    }
    let mut schemas = super::Schemas::new();
    super::named::<first::Same>(&mut schemas);
    super::named::<second::Same>(&mut schemas);
    assert_eq!(clashing_names(schemas), ["Same"]);
}
