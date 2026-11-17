//! Markdown rendering for READMEs, issues, and release notes.

use std::borrow::Cow;

use comrak::{Options, markdown_to_html};

pub(crate) fn render_markdown(markdown: &str) -> String {
    let mut options = Options::default();
    options.extension.strikethrough = true;
    options.extension.table = true;
    options.extension.autolink = true;
    options.extension.tasklist = true;
    options.render.r#unsafe = true;

    let rendered = markdown_to_html(markdown, &options);
    let mut sanitizer = ammonia::Builder::default();
    sanitizer
        .add_generic_attributes(&["align", "class", "id"])
        .add_tags(&["input"])
        .add_tag_attributes("input", &["checked", "disabled", "type"])
        .set_tag_attribute_value("input", "disabled", "")
        .set_tag_attribute_value("input", "type", "checkbox")
        .attribute_filter(filter_markdown_attribute);
    sanitizer.clean(&rendered).to_string()
}

/// Prepended to every `id` in rendered Markdown, and to the in-page links that
/// point at one.
const USER_CONTENT_ID_PREFIX: &str = "user-content-";

/// Rendered Markdown is inserted into the application's own pages, where
/// `class` and `id` mean something: the page's stylesheet applies to any
/// element carrying one of its class names, and its scripts look elements up
/// by id. Markdown may carry raw HTML, so neither is passed through as
/// written.
///
/// The only class the renderer emits is a fenced code block's `language-*`,
/// which the interface highlights by, so that is the only one kept. An `id`
/// stays usable as a link target but is moved into its own namespace, and
/// `#fragment` links are moved with it so they still reach their target.
fn filter_markdown_attribute<'value>(
    element: &str,
    attribute: &str,
    value: &'value str,
) -> Option<Cow<'value, str>> {
    match attribute {
        "class" => {
            if element != "code" {
                return None;
            }
            let languages = value
                .split_ascii_whitespace()
                .filter(|class| {
                    class
                        .strip_prefix("language-")
                        .is_some_and(|language| !language.is_empty())
                })
                .collect::<Vec<_>>();
            (!languages.is_empty()).then(|| Cow::Owned(languages.join(" ")))
        }
        "id" => {
            if value.is_empty() || value.contains(|character: char| character.is_whitespace()) {
                return None;
            }
            Some(user_content_id(value))
        }
        "href" => match value.strip_prefix('#') {
            Some(fragment) if !fragment.is_empty() => {
                Some(Cow::Owned(format!("#{}", user_content_id(fragment))))
            }
            _ => Some(Cow::Borrowed(value)),
        },
        _ => Some(Cow::Borrowed(value)),
    }
}

fn user_content_id(id: &str) -> Cow<'_, str> {
    if id.starts_with(USER_CONTENT_ID_PREFIX) {
        Cow::Borrowed(id)
    } else {
        Cow::Owned(format!("{USER_CONTENT_ID_PREFIX}{id}"))
    }
}

#[cfg(test)]
mod tests {
    use super::render_markdown;

    /// Markdown rendered as it was while every element could keep any `class`
    /// and `id`: what legitimate content must still render to.
    fn render_markdown_unrestricted(markdown: &str) -> String {
        let mut options = comrak::Options::default();
        options.extension.strikethrough = true;
        options.extension.table = true;
        options.extension.autolink = true;
        options.extension.tasklist = true;
        options.render.r#unsafe = true;
        let mut sanitizer = ammonia::Builder::default();
        sanitizer
            .add_generic_attributes(&["align", "class", "id"])
            .add_tags(&["input"])
            .add_tag_attributes("input", &["checked", "disabled", "type"])
            .set_tag_attribute_value("input", "disabled", "")
            .set_tag_attribute_value("input", "type", "checkbox");
        sanitizer
            .clean(&comrak::markdown_to_html(markdown, &options))
            .to_string()
    }

    #[test]
    fn markdown_without_raw_classes_or_ids_renders_as_before() {
        let documents = [
            // Fenced code, with the class the interface highlights by.
            "```rust\nfn main() {}\n```\n\n```c++\nint x;\n```\n\n```\nplain\n```\n\n`inline`",
            // Headings, and a link to a heading.
            "# Title\n\n## Getting started\n\nSee [the start](https://example.test/#getting-started).",
            // Task lists.
            "- [x] done\n- [ ] open\n  - [x] nested",
            // Footnotes and alerts, in the syntax other forges render.
            "Text with a note[^1].\n\n[^1]: The note.",
            "> [!NOTE]\n> Useful information.\n\n> [!WARNING]\n> Careful.",
            // Tables, strikethrough, autolinks, and common README HTML.
            "| Left | Right |\n| :--- | ---: |\n| a | b |\n\n~~gone~~ https://example.test",
            "<p align=\"center\"><img src=\"logo.png\" alt=\"Logo\" width=\"120\"></p>\n\n<details><summary>More</summary>\n\nHidden **text**.\n\n</details>",
        ];
        for document in documents {
            assert_eq!(
                render_markdown(document),
                render_markdown_unrestricted(document),
                "{document}"
            );
        }

        let code = render_markdown("```rust\nfn main() {}\n```");
        assert!(
            code.contains(r#"<pre><code class="language-rust">"#),
            "{code}"
        );
        let tasks = render_markdown("- [x] done\n- [ ] open");
        assert!(
            tasks.contains(r#"<input type="checkbox" checked="" disabled="">"#),
            "{tasks}"
        );
        let table = render_markdown("| Left | Right |\n| :--- | ---: |\n| a | b |");
        assert!(table.contains(r#"<th align="right">Right</th>"#), "{table}");
    }

    #[test]
    fn markdown_cannot_apply_application_classes() {
        let overlay = render_markdown(
            r#"<a href="https://evil.example" class="fixed inset-0 z-50">Sign in again</a>"#,
        );
        assert!(!overlay.contains("class"), "{overlay}");
        assert!(
            overlay.contains(r#"href="https://evil.example""#),
            "{overlay}"
        );
        assert!(overlay.contains("Sign in again"), "{overlay}");

        for html in [
            r#"<div class="fixed inset-0">x</div>"#,
            r#"<span class="hljs-keyword sr-only">x</span>"#,
            r#"<pre class="language-rust hidden">x</pre>"#,
            r#"<code class="hidden">x</code>"#,
            r#"<code class="language-">x</code>"#,
            r#"<img src="a.png" class="size-full">"#,
        ] {
            let rendered = render_markdown(html);
            assert!(!rendered.contains("class"), "{html} -> {rendered}");
        }

        // A code element keeps its language and nothing else.
        let code = render_markdown(r#"<code class="fixed language-rust inset-0">x</code>"#);
        assert!(
            code.contains(r#"<code class="language-rust">x</code>"#),
            "{code}"
        );
    }

    #[test]
    fn markdown_ids_are_namespaced_and_stay_linkable() {
        let rendered = render_markdown(
            "<h2 id=\"install\">Install</h2>\n\n<a id=\"top\"></a>\n\n[Install](#install) and [top](#top), [elsewhere](docs/guide.md#install), [empty](#).",
        );
        assert!(
            rendered.contains(r#"<h2 id="user-content-install">Install</h2>"#),
            "{rendered}"
        );
        assert!(
            rendered.contains(r#"<a id="user-content-top""#),
            "{rendered}"
        );
        assert!(
            rendered.contains("href=\"#user-content-install\""),
            "{rendered}"
        );
        assert!(
            rendered.contains("href=\"#user-content-top\""),
            "{rendered}"
        );
        // Only in-page links move; links to other documents are left alone.
        assert!(
            rendered.contains("href=\"docs/guide.md#install\""),
            "{rendered}"
        );
        assert!(rendered.contains("href=\"#\""), "{rendered}");

        // Ids the application itself uses cannot be claimed.
        let clobber = render_markdown(r#"<div id="app">x</div><form id="login form">y</form>"#);
        assert!(
            clobber.contains(r#"<div id="user-content-app">"#),
            "{clobber}"
        );
        assert!(!clobber.contains("login"), "{clobber}");
        // An id that is already namespaced is not prefixed twice.
        let prefixed = render_markdown("<p id=\"user-content-a\">x</p>\n\n[a](#user-content-a)");
        assert!(prefixed.contains(r#"id="user-content-a""#), "{prefixed}");
        assert!(prefixed.contains("href=\"#user-content-a\""), "{prefixed}");
    }
}
