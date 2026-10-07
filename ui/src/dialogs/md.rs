/// A text somebody else wrote, as HTML.
///
/// One function rather than a `markdown::to_html` at each place text is shown, because the options are the
/// whole point and a call that forgets them looks identical to a call that does not. Everything rendered here
/// arrives the same way — an agent wrote it through `/mcp` — so it is all rendered the same way.
///
/// **GFM, not CommonMark.** `markdown::to_html` is CommonMark, where a pipe table is not a construct at all:
/// the rows are ordinary text, so consecutive lines join into one paragraph and a table of nine networks
/// arrives as a wall of `|` and `---`. That is what a task carrying a table actually looked like. GFM also
/// brings strikethrough, bare URLs as links, and `- [ ]` as a checkbox — all of which agents write.
///
/// **Still escaped.** `Options::gfm()` leaves `allow_dangerous_html` off, so raw HTML in the text is written
/// out as text instead of becoming markup. That is what makes `dangerous_inner_html` safe on the far side; do
/// not turn it on. `gfm_tagfilter` is on top of that.
///
/// **A ```mermaid fence comes out as a diagram**, not as a code block — see [`unwrap_mermaid_blocks`].
pub fn md_to_html(text: &str) -> String {
    // The `Err` arm is unreachable for us: the signature carries MDX's parse errors, and MDX is off. Falling
    // back to the CommonMark render rather than unwrapping, so the worst case is a table that reads badly
    // again rather than a dialog that renders nothing.
    let html = markdown::to_html_with_options(text, &markdown::Options::gfm())
        .unwrap_or_else(|_| markdown::to_html(text));

    unwrap_mermaid_blocks(html)
}

/// `<pre><code class="language-mermaid">…</code></pre>` → `<pre class="mermaid">…</pre>`.
///
/// **The whole of the Rust side of mermaid support**, and deliberately so: `public/assets/mermaid-boot.js`
/// watches the page for `pre.mermaid` and draws whatever it finds, so this is the one line of markup the two
/// sides agree on. No interop, no hook to remember at each of the four places markdown is rendered.
///
/// A rewrite of the rendered html rather than a custom renderer, because the markdown crate has no hook for
/// one fence language — and the output it produces here is fixed and unambiguous: the body is escaped, so
/// `</code></pre>` cannot occur inside a block and the first one always ends it.
///
/// **The body is left escaped**, which is what makes this safe: it stays text, the browser unescapes it into
/// `textContent`, and that is what mermaid parses. Nothing an agent writes in a diagram becomes markup.
///
/// A fence that is not drawn — the library did not load, the page is being read without JavaScript — reads as
/// the diagram's source, which is the honest fallback and is what it looked like before this existed.
fn unwrap_mermaid_blocks(html: String) -> String {
    const OPEN: &str = "<pre><code class=\"language-mermaid\">";
    const CLOSE: &str = "</code></pre>";

    if !html.contains(OPEN) {
        return html;
    }

    let mut out = String::with_capacity(html.len());
    let mut rest = html.as_str();

    while let Some(at) = rest.find(OPEN) {
        out.push_str(&rest[..at]);

        let body = &rest[at + OPEN.len()..];

        // No closing tag is not a shape the renderer produces; passing the remainder through unchanged is
        // the one behaviour that cannot lose somebody's text.
        let Some(end) = body.find(CLOSE) else {
            out.push_str(&rest[at..]);
            return out;
        };

        out.push_str("<pre class=\"mermaid\">");
        out.push_str(&body[..end]);
        out.push_str("</pre>");

        rest = &body[end + CLOSE.len()..];
    }

    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mermaid_fence_becomes_a_diagram_block() {
        let html = md_to_html("```mermaid\ngraph TD;\nA-->B;\n```");

        assert!(
            html.contains("<pre class=\"mermaid\">"),
            "the fence should be handed to mermaid, got: {html}"
        );
        assert!(!html.contains("language-mermaid"));
        assert!(html.contains("graph TD;"));
    }

    /// Every other fence is still a code block — this rewrite is for one language and must not touch the
    /// rest.
    #[test]
    fn other_code_fences_are_untouched() {
        let html = md_to_html("```rust\nfn main() {}\n```");

        assert!(html.contains("language-rust"));
        assert!(!html.contains("class=\"mermaid\""));
    }

    #[test]
    fn two_diagrams_in_one_text_both_come_out() {
        let html = md_to_html("```mermaid\nA\n```\ntext\n```mermaid\nB\n```");

        assert_eq!(html.matches("<pre class=\"mermaid\">").count(), 2);
        assert!(html.contains("text"));
    }

    /// The body stays ESCAPED, which is what keeps `dangerous_inner_html` safe on the far side: an agent
    /// writing a `<script>` into a diagram gets text, and mermaid reads it back as text.
    #[test]
    fn what_is_inside_a_diagram_is_not_markup() {
        let html = md_to_html("```mermaid\ngraph TD;\nA[<script>x</script>]-->B;\n```");

        assert!(!html.contains("<script>"));
        assert!(html.contains("&lt;script&gt;"));
    }

    #[test]
    fn a_text_with_no_diagram_is_left_alone() {
        let plain = md_to_html("# Title\n\nSome *text*.");
        assert!(!plain.contains("mermaid"));
    }
}
