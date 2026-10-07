//! The text mechanics of a document: editing it in place, searching it, reading a slice of it, reading
//! its headings, and diffing two of its versions.
//!
//! **Nothing here knows about `AppContext`, Postgres or MCP.** Every function takes a `&str` and returns a
//! value, which is what makes this the one part of the documents feature that can be tested exhaustively —
//! and it is the part that most needs it. The rules these functions enforce are the difference between an
//! edit that does what was asked and one that quietly changes a document in a way nobody reads back.
//!
//! The wrappers that resolve a document, check it is text and write the new version live in
//! `super::documents`.

use similar::TextDiff;

/// The longest a single line may be in a search result before it is clipped.
///
/// A search over a project can hit a minified json document or a table with a thousand-character row, and
/// one of those lines would be most of the answer. The clip is what keeps "how many documents mention this"
/// a cheap question — which is the entire reason the tool exists.
const MAX_REPORTED_LINE: usize = 300;

/// How many lines around a match are shown when nobody says.
pub const DEFAULT_CONTEXT_LINES: i64 = 2;

/// The most context that can be asked for, per match.
///
/// Beyond this a search stops being a search: ten lines either side of twenty matches is most of a document,
/// and the caller wanted `documents_get` with a slice.
pub const MAX_CONTEXT_LINES: i64 = 10;

/// How many matching lines one document reports when nobody says.
pub const DEFAULT_MAX_MATCHES_PER_DOCUMENT: i64 = 20;

/// The most one document may report, however high the argument is.
pub const MAX_MATCHES_PER_DOCUMENT: i64 = 100;

/// The most matching lines ONE SEARCH may report, across every document it looked in.
///
/// The per-document cap alone does not bound the answer — twenty matches in each of forty documents is
/// eight hundred, which is a context window rather than a search result. The whole is capped too, documents
/// are walked in path order so the cut is deterministic, and the response says it was cut.
pub const MAX_MATCHES_TOTAL: usize = 200;

/// How much context a unified diff carries around each hunk when nobody says.
pub const DEFAULT_DIFF_CONTEXT: i64 = 3;

/// The most context a diff hunk may carry. Past this the hunks merge and the diff is the whole document
/// twice, which is exactly what asking for a diff was meant to avoid.
pub const MAX_DIFF_CONTEXT: i64 = 20;

/// The longest diff that is returned, in bytes.
///
/// Two rewritten versions of a large document produce a diff bigger than either of them, and a diff that
/// does not fit in a tool result is the problem this tool was added to solve rather than a new one. Cut and
/// flagged, so the caller knows to narrow it rather than believing they saw the end.
pub const MAX_DIFF_BYTES: usize = 120_000;

/// The most a batch of edits may grow a text to, in BYTES, before the batch is refused.
///
/// Not a second content limit — `MAX_CONTENT_LEN` is still the one that decides what may be stored, and it
/// counts CHARACTERS. This is the guard that has to fire EARLIER than that one, on the intermediate text,
/// because the thing it is stopping is an allocation rather than a write. Four bytes per character, so a
/// document sitting exactly on the content limit in a four-byte script cannot be refused by this instead.
const MAX_EDITED_BYTES: usize = 4 * super::MAX_CONTENT_LEN;

// ---------------------------------------------------------------------------------------------- editing

/// One replacement, as a caller asks for it.
///
/// The shape is deliberately the same as the edit tool every agent already knows: match an exact piece of
/// the current text and put something else in its place. There is no line number in it, on purpose — a line
/// number goes stale the moment an earlier edit in the same batch changes the length of the document, and
/// the caller would have to compute the drift themselves.
#[derive(Debug, Clone)]
pub struct DocumentEdit {
    pub old_string: String,
    pub new_string: String,
    /// Change EVERY occurrence rather than refusing an ambiguous one. Off by default, and that default is
    /// the whole safety property — see [`apply_edits`].
    pub replace_all: bool,
}

/// Apply a batch of edits to a text, or refuse the batch entirely.
///
/// **All of them or none of them.** The function builds the new text in memory and only the caller writes
/// it, so a refusal at edit five leaves nothing behind from edits one to four. That is not a nicety: a
/// half-applied batch is a document in a state nobody asked for, and the caller cannot tell how far it got
/// without reading the whole thing back.
///
/// **Edits are applied IN ORDER, each against the result of the one before.** So an edit may legitimately
/// match text that a previous edit produced, and an edit whose `old_string` a previous edit destroyed is an
/// error rather than a silent skip. The alternative — matching everything against the original text and
/// splicing at the end — cannot express "rename this, then adjust the line I just wrote", and quietly
/// produces nonsense when two edits overlap.
///
/// **An ambiguous match is refused rather than resolved.** If `old_string` appears more than once and
/// `replace_all` was not asked for, the batch fails and says how many times it was found. This is the rule
/// that matters most in a large document: silently taking the first occurrence, or silently taking all of
/// them, is exactly how a specification ends up subtly wrong in a place nobody looks at again.
///
/// Returns the new text and, per edit and in order, how many occurrences it changed.
pub fn apply_edits(text: &str, edits: &[DocumentEdit]) -> Result<(String, Vec<i32>), String> {
    if edits.is_empty() {
        return Err(
            "no edits to make — pass at least one `{ old_string, new_string }` in `edits`".to_string(),
        );
    }

    let mut current = text.to_string();
    let mut replacements = Vec::with_capacity(edits.len());

    for (index, edit) in edits.iter().enumerate() {
        let ordinal = index + 1;

        if edit.old_string.is_empty() {
            return Err(format!(
                "edit {ordinal} has an empty `old_string`, which matches no position at all. To ADD text, \
                 match the line it goes next to and write that line plus the new one into `new_string`"
            ));
        }

        if edit.old_string == edit.new_string {
            return Err(format!(
                "edit {ordinal} replaces {} with itself. NOTHING WAS WRITTEN — a no-op inside a batch is \
                 almost always a copy-paste that lost its change",
                preview(&edit.old_string)
            ));
        }

        // Counted the same way `replace` replaces: non-overlapping, left to right. Any other counting rule
        // would let the message disagree with what the write then did.
        let found = current.matches(edit.old_string.as_str()).count();

        if found == 0 {
            // Which text it was matched against is the useful half here: an edit that would have matched the
            // document as it was read can still miss because an earlier edit in the same batch rewrote that
            // very piece, and those two failures have different fixes.
            let against = if index == 0 {
                String::new()
            } else {
                format!(" (matched against the text as edit {index} left it)")
            };

            return Err(format!(
                "edit {ordinal}: {} is not in the document{against}. NOTHING WAS WRITTEN — all {} edits \
                 were refused together. Read the document again and copy the text exactly: the match is \
                 literal, so indentation, trailing spaces and the kind of dash all count",
                preview(&edit.old_string),
                edits.len()
            ));
        }

        if found > 1 && !edit.replace_all {
            return Err(format!(
                "edit {ordinal}: {} appears {found} times, so it does not say WHICH one to change. NOTHING \
                 WAS WRITTEN. Either give it more surrounding text until it matches once, or pass \
                 `replace_all: true` if all {found} really should change. Quietly changing every occurrence \
                 in a large document is the way one gets wrong without anybody noticing",
                preview(&edit.old_string)
            ));
        }

        // Refused BEFORE the allocation, and that ordering is the whole of it.
        //
        // `replace_all` multiplies: every occurrence grows by the difference between the two strings, so a
        // one-megabyte document of `a` and a one-kilobyte `new_string` asks for a gigabyte — and a batch
        // compounds, because each edit runs against the result of the last. The content limit cannot catch
        // it: `DocumentBody::validate` runs on the finished text, by which time the allocation has already
        // been attempted. An allocation that fails in Rust ABORTS the process; it is not an error a tool
        // call can return. So the whole service would die of one edit, and this is the check that stops it.
        let growth = found as i64 * (edit.new_string.len() as i64 - edit.old_string.len() as i64);
        let projected = current.len() as i64 + growth;

        if projected > MAX_EDITED_BYTES as i64 {
            return Err(format!(
                "edit {ordinal} would grow the document to about {projected} bytes, past the {MAX_EDITED_BYTES} \
                 this allows. NOTHING WAS WRITTEN. {} occurrence(s) each growing by {} bytes is what does it — \
                 with `replace_all` on a large document that multiplies fast. Narrow the match, or split the \
                 change across documents",
                found,
                edit.new_string.len() as i64 - edit.old_string.len() as i64
            ));
        }

        current = if edit.replace_all {
            current.replace(edit.old_string.as_str(), &edit.new_string)
        } else {
            current.replacen(edit.old_string.as_str(), &edit.new_string, 1)
        };

        replacements.push(found as i32);
    }

    Ok((current, replacements))
}

/// A piece of text as it appears inside an error message: one line, escaped, and short.
///
/// An `old_string` can be a whole paragraph, and pasting one back into an error makes the error unreadable
/// at exactly the moment somebody needs to read it. Newlines and tabs are escaped rather than dropped
/// because they are usually the reason the match failed.
fn preview(text: &str) -> String {
    const LIMIT: usize = 60;

    let mut flat = String::with_capacity(LIMIT + 8);

    for ch in text.chars().take(LIMIT) {
        match ch {
            '\n' => flat.push_str("\\n"),
            '\r' => flat.push_str("\\r"),
            '\t' => flat.push_str("\\t"),
            _ => flat.push(ch),
        }
    }

    if text.chars().nth(LIMIT).is_some() {
        format!("`{flat}…`")
    } else {
        format!("`{flat}`")
    }
}

// -------------------------------------------------------------------------------------------- searching

/// A compiled query — the one thing a search over many documents should build once.
#[derive(Debug)]
pub struct Matcher {
    regex: regex::Regex,
}

impl Matcher {
    /// Compile a query, or say why it is not one.
    ///
    /// **Both kinds of query become a regex**, because a literal one is a regex with `regex::escape` around
    /// it and the engine compiles a literal down to a substring scan anyway. One code path rather than two
    /// means case-insensitivity, the line walk and the caps cannot drift apart between them.
    ///
    /// The size limit is the guard against a pattern that is valid and enormous — `a{1000}{1000}` compiles
    /// to a program that would eat the process. The engine itself is why there is no TIME limit to set:
    /// matching is linear in the length of the line, whatever the pattern.
    pub fn new(query: &str, is_regex: bool, case_sensitive: bool) -> Result<Self, String> {
        if query.is_empty() {
            return Err("nothing to search for — pass a `query`".to_string());
        }

        // NOT trimmed: a leading or trailing space is a legitimate thing to search for, and trimming it
        // would silently answer a different question than the one asked.
        let pattern = if is_regex {
            query.to_string()
        } else {
            regex::escape(query)
        };

        let regex = regex::RegexBuilder::new(&pattern)
            .case_insensitive(!case_sensitive)
            .size_limit(1 << 20)
            .build()
            .map_err(|err| {
                format!(
                    "`{query}` is not a valid regular expression: {err}\nPass `is_regex: false` to search \
                     for it as literal text"
                )
            })?;

        Ok(Self { regex })
    }

    pub fn is_match(&self, line: &str) -> bool {
        self.regex.is_match(line)
    }
}

/// One line that matched, with the lines around it.
#[derive(Debug, Clone)]
pub struct TextMatch {
    /// 1-based, so it goes straight into `from_line` on a read.
    pub line_number: i64,
    pub line: String,
    pub context_before: Vec<String>,
    pub context_after: Vec<String>,
}

/// Every line of one text that matches, up to a cap.
///
/// **Line-oriented, exactly like grep**, and that is a real limit rather than an oversight: a pattern
/// cannot match across a newline. It is what makes the cost of a search proportional to the text rather
/// than to the pattern, and it is what makes a result addressable — a line number is something the caller
/// can hand straight back to `documents_get` as `from_line`.
///
/// A line that matches twice is ONE result. What is being counted is lines worth reading, not occurrences.
///
/// Returns the matches kept and how many lines matched in total — the second number is what tells a caller
/// its cap hid something, which an empty tail could not.
pub fn search_text(
    matcher: &Matcher,
    text: &str,
    context_lines: usize,
    max_matches: usize,
) -> (Vec<TextMatch>, i32) {
    let lines: Vec<&str> = text.lines().collect();

    let mut kept: Vec<TextMatch> = Vec::new();
    let mut total = 0;

    for (index, line) in lines.iter().enumerate() {
        if !matcher.is_match(line) {
            continue;
        }

        total += 1;

        // Counting carries on past the cap: "20 of 57" is a different instruction to the reader than "20",
        // and the walk is cheap compared with what has already been paid to get the text here.
        if kept.len() >= max_matches {
            continue;
        }

        let from = index.saturating_sub(context_lines);
        let to = (index + context_lines + 1).min(lines.len());

        kept.push(TextMatch {
            line_number: index as i64 + 1,
            line: clip_chars(line, MAX_REPORTED_LINE),
            context_before: lines[from..index]
                .iter()
                .map(|itm| clip_chars(itm, MAX_REPORTED_LINE))
                .collect(),
            context_after: lines[index + 1..to]
                .iter()
                .map(|itm| clip_chars(itm, MAX_REPORTED_LINE))
                .collect(),
        });
    }

    (kept, total)
}

// ---------------------------------------------------------------------------------------------- slicing

/// A piece of a document, and where in it that piece came from.
#[derive(Debug, Clone, PartialEq)]
pub struct TextSlice {
    /// EXACTLY the bytes that are stored for those lines, terminators and all — not a re-join. See
    /// [`line_spans`] for why that distinction is load-bearing.
    pub text: String,
    /// 1-based and inclusive. Both are 0 for an empty document, which has no line 1 to name.
    pub from_line: i64,
    /// The last line returned WHOLE. Normally at or after `from_line` — but `from_line - 1` when a byte
    /// budget could not fit even the first line, so that `text` holds part of a line that no number claims.
    /// Reading on from `to_line + 1` is correct either way, which is the point of reporting it like this:
    /// the partially-shown line gets read again rather than skipped.
    pub to_line: i64,
    pub lines_total: i64,
    /// True when what is in `text` is not the whole document — by line range, by byte budget, or both.
    pub truncated: bool,
}

/// Read part of a text, by line range and/or a byte budget.
///
/// Both bounds are **1-based and inclusive**, which is how every editor, every stack trace and every
/// `outline` entry numbers lines. Off-by-one here would be paid on every call by a caller converting.
///
/// `max_bytes` cuts **at a line boundary** wherever one fits: half a line of Markdown is not something a
/// reader can reason about, and a table row cut in the middle reads as a different row. When not even the
/// first line fits, it cuts inside that line at a character boundary rather than returning nothing — an
/// empty answer to "give me some of this" is worse than a partial one.
pub fn slice_text(
    text: &str,
    from_line: Option<i64>,
    to_line: Option<i64>,
    max_bytes: Option<i64>,
) -> Result<TextSlice, String> {
    let spans = line_spans(text);
    let lines_total = spans.len() as i64;

    if let Some(max) = max_bytes
        && max < 1
    {
        return Err(format!(
            "`max_bytes` is {max} — ask for at least one byte, or omit it for the whole document"
        ));
    }

    let from = from_line.unwrap_or(1);

    if from < 1 {
        return Err(format!(
            "`from_line` is {from}, and lines are numbered from 1 — the same numbers documents_outline and \
             documents_search report"
        ));
    }

    // Only a bound the CALLER wrote is checked against `from`. The default end is the last line of the
    // document, which is behind `from` whenever the document is short — and reporting that as a backwards
    // range would answer a question nobody asked, hiding the two real messages below.
    if let Some(to) = to_line
        && to < from
    {
        return Err(format!("`to_line` ({to}) is before `from_line` ({from})"));
    }

    if lines_total == 0 {
        return Ok(TextSlice {
            text: String::new(),
            from_line: 0,
            to_line: 0,
            lines_total: 0,
            truncated: false,
        });
    }

    if from > lines_total {
        return Err(format!(
            "`from_line` is {from} and the document has {lines_total} lines"
        ));
    }

    // Asking past the end is not an error — it is how "give me everything from here" is written when the
    // caller does not know the length.
    let to = to_line.unwrap_or(lines_total).min(lines_total);

    let start = spans[(from - 1) as usize].0;

    let mut body = &text[start..line_range_end(text, &spans, to, lines_total)];
    let mut last = to;
    let mut cut = false;

    if let Some(max) = max_bytes {
        let max = max as usize;

        if body.len() > max {
            cut = true;

            // The last WHOLE line that fits, found by walking the ends rather than by adding up lengths:
            // the terminators are part of the text now, so their cost is already in the offsets.
            let mut fitting = 0;

            for line in from..=to {
                if line_range_end(text, &spans, line, lines_total) - start > max {
                    break;
                }

                fitting += 1;
            }

            if fitting == 0 {
                // Not even the first line fits. A partial line is still worth returning — but `to_line`
                // must NOT claim this line, or the documented "read on from `to_line` + 1" would skip the
                // part that was cut off and the caller would never learn it existed. `from_line - 1` says
                // no line came back whole, and makes that continuation re-read this one.
                body = &text[start..start + fitting_bytes(text, start, max)];
                last = from - 1;

                if body.is_empty() {
                    return Err(format!(
                        "`max_bytes` is {max}, which is smaller than the first character of line {from} — \
                         there is nothing that could be returned. Raise it, or ask for a later line"
                    ));
                }
            } else {
                last = from + fitting - 1;
                body = &text[start..line_range_end(text, &spans, last, lines_total)];
            }
        }
    }

    Ok(TextSlice {
        text: body.to_string(),
        from_line: from,
        to_line: last,
        lines_total,
        truncated: cut || from > 1 || last < lines_total,
    })
}

/// Where the text of lines `..=line` ends, in bytes.
///
/// The LAST line of the document ends at the end of the text — terminator included — so that reading a
/// whole document hands back exactly the bytes that are stored, trailing newline and all. Any other line
/// ends where its own text does, so a slice out of the middle has no dangling terminator on it.
fn line_range_end(text: &str, spans: &[(usize, usize)], line: i64, lines_total: i64) -> usize {
    if line >= lines_total {
        text.len()
    } else {
        spans[(line - 1) as usize].1
    }
}

/// How many bytes from `start` fit in `max`, cut at a character boundary.
fn fitting_bytes(text: &str, start: usize, max: usize) -> usize {
    let mut cut = max.min(text.len() - start);

    while cut > 0 && !text.is_char_boundary(start + cut) {
        cut -= 1;
    }

    cut
}

/// Where each line of a text begins and ends, in BYTES, with the terminator excluded from the end.
///
/// **Built rather than reached for via `str::lines` and a re-join, and the difference is not cosmetic.**
/// `lines` throws away which terminator each line had, so joining them back with `"\n"` produces a document
/// that is not the one stored: every `\r\n` silently becomes `\n` and a trailing newline disappears. A read
/// that did that would report `truncated: false` while handing back different bytes — and a caller copying
/// two of those lines into `documents_edit.old_string` would be told the text is not in the document, with
/// the error pointing at indentation and dashes rather than at the line endings.
///
/// The split matches `str::lines` exactly, so line NUMBERS agree everywhere: split on `\n`, drop one
/// preceding `\r`, and no empty final line when the text ends in a terminator.
fn line_spans(text: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut start = 0;

    for (at, ch) in text.char_indices() {
        if ch != '\n' {
            continue;
        }

        let mut end = at;

        if end > start && text.as_bytes()[end - 1] == b'\r' {
            end -= 1;
        }

        spans.push((start, end));
        start = at + 1;
    }

    // Whatever is after the last terminator, when the text does not end with one.
    if start < text.len() {
        spans.push((start, text.len()));
    }

    spans
}

// --------------------------------------------------------------------------------------------- outlines

/// One heading, and the span of document it opens.
#[derive(Debug, Clone, PartialEq)]
pub struct DocumentHeading {
    /// 1 for `#`, 6 for `######`.
    pub level: i32,
    pub title: String,
    /// The line the heading itself is on, 1-based.
    pub line: i64,
    /// The last line of this section, inclusive — the line before the next heading at this level or above,
    /// or the last line of the document.
    pub end_line: i64,
    /// How many bytes that span is, so a caller can tell a section worth reading whole from one that needs
    /// its own slice.
    pub size: i64,
}

/// The Markdown headings of a text, in the order they appear.
///
/// **Fenced code blocks are skipped**, and that is the rule that makes this worth having rather than a
/// three-line `filter`. A specification full of shell examples has `# rebuild the image` inside a fence on
/// nearly every page, and an outline that reported those would be longer than the document's real structure
/// and would name sections that do not exist.
///
/// ATX headings only — `#` through `######`. Setext (`===` under a line) is deliberately not recognised:
/// `---` is also a thematic break and a front-matter fence, so reading it as a heading turns every document
/// with front matter into one whose first section is called by its first key.
///
/// **A section CONTAINS its subsections**, so `end_line` runs to the next heading of the same level or
/// shallower and the spans of nested headings overlap. That is what makes a slice of one entry a slice of
/// the whole section rather than of its first paragraph.
pub fn outline_of(text: &str) -> Vec<DocumentHeading> {
    let spans = line_spans(text);

    let mut headings: Vec<DocumentHeading> = Vec::new();
    let mut fence: Option<(char, usize)> = None;

    for (index, span) in spans.iter().enumerate() {
        let raw = &text[span.0..span.1];
        let line = raw.trim_start();

        // CHARACTERS, not bytes. Measured in bytes, two non-breaking spaces are four — so a heading
        // indented by them reads as an indented code block and vanishes from the outline entirely, which
        // an agent can only interpret as the section not existing.
        let indent = raw.chars().count() - line.chars().count();

        // Four columns in is an indented code block, where nothing is markup.
        if indent > 3 {
            continue;
        }

        if let Some(marker) = fence_marker(line) {
            match fence {
                None => fence = Some(marker),
                // A closing fence is the same character, at least as long, and has nothing after it — which
                // is what tells it from an opening fence carrying a language name.
                Some((ch, len)) => {
                    if marker.0 == ch && marker.1 >= len && line[marker.1..].trim().is_empty() {
                        fence = None;
                    }
                }
            }

            continue;
        }

        if fence.is_some() {
            continue;
        }

        let Some((level, title)) = atx_heading(line) else {
            continue;
        };

        headings.push(DocumentHeading {
            level,
            title: title.to_string(),
            line: index as i64 + 1,
            // Filled in below, once the next heading at this level is known.
            end_line: 0,
            size: 0,
        });
    }

    let lines_total = spans.len() as i64;

    for index in 0..headings.len() {
        let level = headings[index].level;

        let end = headings[index + 1..]
            .iter()
            .find(|itm| itm.level <= level)
            .map(|itm| itm.line - 1)
            .unwrap_or(lines_total)
            .max(headings[index].line);

        headings[index].end_line = end;

        // From the real byte offsets, so the size counts the terminators the document actually has rather
        // than assuming one byte each — a CRLF document would otherwise under-report every section.
        headings[index].size = (line_range_end(text, &spans, end, lines_total)
            - spans[(headings[index].line - 1) as usize].0) as i64;
    }

    headings
}

/// The fence a line opens or closes, if it is one: three or more backticks or tildes.
fn fence_marker(line: &str) -> Option<(char, usize)> {
    let first = line.chars().next()?;

    if first != '`' && first != '~' {
        return None;
    }

    let length = line.chars().take_while(|ch| *ch == first).count();

    if length < 3 {
        return None;
    }

    Some((first, length))
}

/// The level and title of an ATX heading, if the line is one.
fn atx_heading(line: &str) -> Option<(i32, &str)> {
    if !line.starts_with('#') {
        return None;
    }

    let hashes = line.chars().take_while(|ch| *ch == '#').count();

    if hashes > 6 {
        return None;
    }

    let rest = &line[hashes..];

    // A space after the hashes is required, which is what keeps `#hashtag` and `#1` out of an outline.
    if !rest.is_empty() && !rest.starts_with(' ') && !rest.starts_with('\t') {
        return None;
    }

    let mut title = rest.trim();

    // A closing run of hashes is decoration and not part of the name — but only when a space precedes it,
    // or `C#` would come back as `C`.
    if title.ends_with('#') {
        let stripped = title.trim_end_matches('#');

        if stripped.is_empty() || stripped.ends_with(' ') {
            title = stripped.trim_end();
        }
    }

    Some((hashes as i32, title))
}

// ---------------------------------------------------------------------------------------------- diffing

/// A unified diff of two texts, and whether it had to be cut short.
///
/// Line-based, because a document is read as lines and a character-level diff of prose is unreadable: a
/// reworded paragraph comes back as a scatter of single letters rather than as one line replaced.
///
/// The labels are written into the `---` / `+++` header, which is what makes a diff of a document that
/// MOVED legible: the two paths differ, and the header is the only place that shows it.
pub fn unified_diff(
    before: &str,
    after: &str,
    before_label: &str,
    after_label: &str,
    context_lines: usize,
) -> (String, bool) {
    let diff = TextDiff::from_lines(before, after);

    let mut unified = diff.unified_diff();

    let text = unified
        .context_radius(context_lines)
        .header(before_label, after_label)
        .to_string();

    if text.len() <= MAX_DIFF_BYTES {
        return (text, false);
    }

    (clip_bytes(&text, MAX_DIFF_BYTES), true)
}

// --------------------------------------------------------------------------------------------- clipping

/// At most `limit` CHARACTERS, with an ellipsis when something was dropped.
fn clip_chars(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }

    let clipped: String = text.chars().take(limit).collect();

    format!("{clipped}…")
}

/// At most `limit` BYTES, cut at a character boundary.
///
/// Bytes rather than characters because what is being bounded is the size of a response, and a naive
/// `&text[..limit]` panics the moment the limit lands inside a multi-byte character — which in a document
/// full of em-dashes is most limits.
fn clip_bytes(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_string();
    }

    let mut cut = limit;

    while cut > 0 && !text.is_char_boundary(cut) {
        cut -= 1;
    }

    text[..cut].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edit(old: &str, new: &str) -> DocumentEdit {
        DocumentEdit {
            old_string: old.to_string(),
            new_string: new.to_string(),
            replace_all: false,
        }
    }

    fn edit_all(old: &str, new: &str) -> DocumentEdit {
        DocumentEdit {
            replace_all: true,
            ..edit(old, new)
        }
    }

    // ------------------------------------------------------------------------------------- editing

    #[test]
    fn one_edit_replaces_one_thing() {
        let (text, counts) = apply_edits("hello world", &[edit("world", "there")]).unwrap();

        assert_eq!(text, "hello there");
        assert_eq!(counts, vec![1]);
    }

    /// The ordering rule, and the reason it is the ordering rule: edit two matches text edit one wrote.
    #[test]
    fn edits_apply_in_order_each_onto_the_last() {
        let (text, counts) =
            apply_edits("a", &[edit("a", "b"), edit("b", "c")]).unwrap();

        assert_eq!(text, "c");
        assert_eq!(counts, vec![1, 1]);
    }

    /// The property the whole tool turns on. Edit one is perfectly good and must still leave no trace.
    #[test]
    fn a_batch_that_fails_anywhere_changes_nothing() {
        let result = apply_edits(
            "alpha beta",
            &[edit("alpha", "ALPHA"), edit("gamma", "GAMMA")],
        );

        let message = result.unwrap_err();

        assert!(message.contains("edit 2"), "it must say which edit: {message}");
        assert!(
            message.contains("NOTHING WAS WRITTEN"),
            "it must say the batch was refused: {message}"
        );
    }

    /// The rule that stops a document going quietly wrong.
    #[test]
    fn an_ambiguous_match_is_refused_and_counted() {
        let message = apply_edits("x y x", &[edit("x", "z")]).unwrap_err();

        assert!(message.contains("appears 2 times"), "{message}");
        assert!(message.contains("replace_all"), "it must name the way out: {message}");
    }

    #[test]
    fn replace_all_takes_every_occurrence_and_reports_how_many() {
        let (text, counts) = apply_edits("x y x", &[edit_all("x", "z")]).unwrap();

        assert_eq!(text, "z y z");
        assert_eq!(counts, vec![2]);
    }

    /// `replace_all` on something that is not there is still an error: the caller believes it changed
    /// something, and a batch that reports success having done nothing is the worst possible answer.
    #[test]
    fn replace_all_does_not_excuse_a_missing_match() {
        assert!(apply_edits("a b", &[edit_all("q", "z")]).is_err());
    }

    #[test]
    fn an_empty_or_no_op_edit_is_refused() {
        assert!(apply_edits("abc", &[edit("", "x")]).is_err());
        assert!(apply_edits("abc", &[edit("abc", "abc")]).is_err());
        assert!(apply_edits("abc", &[]).is_err());
    }

    /// The batch is atomic, so the message has to say WHERE it failed — and, when an earlier edit is the
    /// reason, that the document as read is not what edit two was matched against.
    #[test]
    fn a_match_destroyed_by_an_earlier_edit_says_so() {
        let message = apply_edits("hello", &[edit("hello", "bye"), edit("hello", "hi")]).unwrap_err();

        assert!(message.contains("edit 1 left it"), "{message}");
    }

    /// Growing a match must not re-match what it just wrote — `str::replace` scans the original, and this
    /// pins that it stays that way.
    #[test]
    fn a_replacement_containing_the_match_does_not_run_away() {
        let (text, _) = apply_edits("foo", &[edit_all("foo", "foofoo")]).unwrap();

        assert_eq!(text, "foofoo");
    }

    /// THE ONE THAT KILLS THE PROCESS IF IT REGRESSES.
    ///
    /// `replace_all` multiplies by the number of occurrences, and a batch compounds because each edit runs
    /// against the result of the last. The content limit cannot catch it — that runs on the finished text,
    /// after the allocation has been attempted — and a failed allocation in Rust ABORTS rather than
    /// returning an error, so the whole service goes down on one tool call. Refused before allocating.
    #[test]
    fn an_edit_that_would_explode_the_document_is_refused_before_it_allocates() {
        // 200k occurrences, each growing by ~1 KB: about 200 MB, from a call of a few kilobytes.
        let document = "a".repeat(200_000);
        let payload = "x".repeat(1_000);

        let message = apply_edits(&document, &[edit_all("a", &payload)]).unwrap_err();

        assert!(message.contains("NOTHING WAS WRITTEN"), "{message}");
        assert!(message.contains("grow the document"), "{message}");
    }

    /// The compounding case: each edit on its own is modest, and together they are not.
    #[test]
    fn growth_is_measured_against_the_running_text_not_the_original() {
        let document = "ab".repeat(50_000);

        let message = apply_edits(
            &document,
            &[edit_all("a", &"a".repeat(50)), edit_all("b", &"b".repeat(500))],
        )
        .unwrap_err();

        assert!(message.contains("edit 2"), "the first edit is affordable: {message}");
    }

    /// And the guard must not refuse ordinary work — a real rename in a large document.
    #[test]
    fn an_ordinary_replace_all_is_not_refused() {
        let document = "the API is an API\n".repeat(1_000);

        let (text, counts) = apply_edits(&document, &[edit_all("API", "interface")]).unwrap();

        assert_eq!(counts, vec![2_000]);
        assert!(text.contains("the interface is an interface"));
    }

    #[test]
    fn a_multi_line_match_works_and_is_escaped_in_the_error() {
        let (text, _) = apply_edits("a\nb\nc", &[edit("a\nb", "A\nB")]).unwrap();
        assert_eq!(text, "A\nB\nc");

        let message = apply_edits("x", &[edit("q\nr", "z")]).unwrap_err();
        assert!(message.contains("q\\nr"), "the newline must be escaped: {message}");
    }

    // ----------------------------------------------------------------------------------- searching

    #[test]
    fn a_literal_query_is_not_read_as_a_pattern() {
        let matcher = Matcher::new("a.c", false, true).unwrap();

        assert!(matcher.is_match("a.c"));
        assert!(!matcher.is_match("abc"), "the dot must be literal");
    }

    #[test]
    fn a_regex_query_is() {
        let matcher = Matcher::new("a.c", true, true).unwrap();

        assert!(matcher.is_match("abc"));
    }

    #[test]
    fn case_insensitive_is_the_default_way_round() {
        assert!(Matcher::new("FOO", false, false).unwrap().is_match("foo"));
        assert!(!Matcher::new("FOO", false, true).unwrap().is_match("foo"));
    }

    #[test]
    fn a_broken_pattern_is_refused_with_the_way_out() {
        let message = Matcher::new("a(", true, false).unwrap_err();

        assert!(message.contains("is_regex: false"), "{message}");
    }

    #[test]
    fn a_match_carries_its_line_number_and_its_context() {
        let matcher = Matcher::new("needle", false, false).unwrap();
        let (matches, total) = search_text(&matcher, "a\nb\nneedle\nd\ne", 1, 10);

        assert_eq!(total, 1);
        assert_eq!(matches[0].line_number, 3);
        assert_eq!(matches[0].line, "needle");
        assert_eq!(matches[0].context_before, vec!["b"]);
        assert_eq!(matches[0].context_after, vec!["d"]);
    }

    /// Context at the edges is short rather than padded, and must not panic.
    #[test]
    fn context_at_the_first_and_last_line_is_what_there_is() {
        let matcher = Matcher::new("x", false, false).unwrap();
        let (matches, _) = search_text(&matcher, "x\nb\nx", 5, 10);

        assert!(matches[0].context_before.is_empty());
        assert_eq!(matches[0].context_after, vec!["b", "x"]);
        assert!(matches[1].context_after.is_empty());
    }

    /// The count keeps going past the cap: "3 of 5" is what tells a caller to narrow the query.
    #[test]
    fn the_cap_limits_what_is_returned_and_not_what_is_counted() {
        let matcher = Matcher::new("x", false, false).unwrap();
        let (matches, total) = search_text(&matcher, "x\nx\nx\nx\nx", 0, 3);

        assert_eq!(matches.len(), 3);
        assert_eq!(total, 5);
    }

    /// A line that matches twice is one result — what is counted is lines worth reading.
    #[test]
    fn a_line_matching_twice_is_reported_once() {
        let matcher = Matcher::new("x", false, false).unwrap();
        let (matches, total) = search_text(&matcher, "x and x", 0, 10);

        assert_eq!(matches.len(), 1);
        assert_eq!(total, 1);
    }

    #[test]
    fn a_very_long_line_is_clipped_rather_than_returned_whole() {
        let matcher = Matcher::new("needle", false, false).unwrap();
        let long = format!("needle{}", "a".repeat(5_000));
        let (matches, _) = search_text(&matcher, &long, 0, 10);

        assert!(matches[0].line.chars().count() <= MAX_REPORTED_LINE + 1);
        assert!(matches[0].line.ends_with('…'));
    }

    // ------------------------------------------------------------------------------------- slicing

    #[test]
    fn a_slice_is_one_based_and_inclusive_at_both_ends() {
        let slice = slice_text("a\nb\nc\nd", Some(2), Some(3), None).unwrap();

        assert_eq!(slice.text, "b\nc");
        assert_eq!((slice.from_line, slice.to_line), (2, 3));
        assert_eq!(slice.lines_total, 4);
        assert!(slice.truncated);
    }

    #[test]
    fn no_bounds_is_the_whole_document_and_is_not_truncated() {
        let slice = slice_text("a\nb", None, None, None).unwrap();

        assert_eq!(slice.text, "a\nb");
        assert!(!slice.truncated);
    }

    /// Asking past the end is how "everything from here" is written by a caller who does not know the
    /// length. Starting past the end is a different thing and IS an error.
    #[test]
    fn an_end_past_the_document_clamps_but_a_start_past_it_does_not() {
        let slice = slice_text("a\nb", Some(2), Some(900), None).unwrap();
        assert_eq!(slice.text, "b");
        assert_eq!(slice.to_line, 2);

        let message = slice_text("a\nb", Some(9), None, None).unwrap_err();
        assert!(message.contains("has 2 lines"), "{message}");
    }

    #[test]
    fn a_backwards_or_zero_range_is_refused() {
        assert!(slice_text("a\nb\nc", Some(3), Some(2), None).is_err());
        assert!(slice_text("a\nb\nc", Some(0), None, None).is_err());
        assert!(slice_text("a\nb\nc", None, None, Some(0)).is_err());
    }

    /// The byte budget stops at a whole line wherever one fits.
    #[test]
    fn a_byte_budget_cuts_at_a_line_boundary() {
        // "aaa\nbbb" is 7 bytes; 5 pays for the first line and not the second.
        let slice = slice_text("aaa\nbbb\nccc", None, None, Some(5)).unwrap();

        assert_eq!(slice.text, "aaa");
        assert_eq!(slice.to_line, 1);
        assert!(slice.truncated);
    }

    /// When not even the first line fits, half a line beats nothing — and must not split a character.
    ///
    /// `to_line` reports the line BEFORE the range, which is the whole subtlety: no line came back whole,
    /// so continuing from `to_line + 1` re-reads this one instead of skipping the part that was cut off.
    #[test]
    fn a_budget_smaller_than_the_first_line_cuts_inside_it_safely() {
        let slice = slice_text("émdash——————\nsecond", None, None, Some(3)).unwrap();

        assert_eq!(slice.text, "ém", "cut at a character boundary, not mid-é");
        assert!(slice.truncated);
        assert_eq!(
            slice.to_line, 0,
            "no whole line was returned, so continuing from to_line + 1 re-reads line 1"
        );
    }

    /// A budget below the first CHARACTER can return nothing at all, and nothing is not an answer — the
    /// caller would read `text: \"\"` as an empty document.
    #[test]
    fn a_budget_below_the_first_character_is_refused() {
        let message = slice_text("émdash", None, None, Some(1)).unwrap_err();

        assert!(message.contains("smaller than the first character"), "{message}");
    }

    /// THE ROUND TRIP. A document read whole must come back byte for byte, or text copied out of a read
    /// and into documents_edit.old_string will not match what is stored — and the edit's error message
    /// sends the caller looking at indentation and dashes instead of at line endings.
    #[test]
    fn a_whole_read_returns_exactly_what_is_stored() {
        for stored in [
            "a\r\nb\r\nc\r\n",
            "a\r\nb\r\nc",
            "a\nb\nc\n",
            "a\nb\nc",
            "\n\n\n",
            "one line",
            "trailing\n",
            "é—😀\r\nsecond\r\n",
        ] {
            let slice = slice_text(stored, None, None, None).unwrap();

            assert_eq!(slice.text, stored, "{stored:?} did not round-trip");
            assert!(!slice.truncated, "{stored:?} reads as the whole document");
        }
    }

    /// A slice out of the MIDDLE keeps the terminators it had, and carries no dangling one.
    #[test]
    fn a_middle_slice_keeps_the_documents_own_line_endings() {
        let slice = slice_text("a\r\nb\r\nc\r\nd\r\n", Some(2), Some(3), None).unwrap();

        assert_eq!(slice.text, "b\r\nc");
        assert!(slice.truncated);
    }

    /// Line NUMBERS must agree with str::lines everywhere, or a number from documents_search or
    /// documents_outline would read a different line back.
    #[test]
    fn line_numbering_agrees_with_str_lines() {
        for text in ["", "\n", "\n\n\n", "a", "a\n", "a\r\nb", "a\r\nb\r\n", "\r\n"] {
            assert_eq!(
                line_spans(text).len(),
                text.lines().count(),
                "{text:?} counts differently"
            );

            for (index, line) in text.lines().enumerate() {
                let span = line_spans(text)[index];
                assert_eq!(&text[span.0..span.1], line, "{text:?} line {index}");
            }
        }
    }

    #[test]
    fn an_empty_document_slices_to_nothing_rather_than_failing() {
        let slice = slice_text("", None, None, None).unwrap();

        assert_eq!(slice.lines_total, 0);
        assert_eq!((slice.from_line, slice.to_line), (0, 0));
        assert!(!slice.truncated);
    }

    // ------------------------------------------------------------------------------------ outlines

    #[test]
    fn headings_come_back_with_their_levels_and_line_numbers() {
        let outline = outline_of("# One\ntext\n## Two\nmore\n# Three");

        assert_eq!(outline.len(), 3);
        assert_eq!((outline[0].level, outline[0].title.as_str(), outline[0].line), (1, "One", 1));
        assert_eq!((outline[1].level, outline[1].title.as_str(), outline[1].line), (2, "Two", 3));
        assert_eq!((outline[2].level, outline[2].title.as_str(), outline[2].line), (1, "Three", 5));
    }

    /// A section runs to the next heading at its level or above, so it contains its subsections — which is
    /// what makes one entry sliceable as a whole section.
    #[test]
    fn a_section_contains_its_subsections() {
        let outline = outline_of("# One\na\n## Two\nb\n# Three\nc");

        assert_eq!(outline[0].end_line, 4, "One runs up to the line before Three");
        assert_eq!(outline[1].end_line, 4, "Two runs to the same place");
        assert_eq!(outline[2].end_line, 6, "the last one runs to the end");
    }

    /// The rule that makes this worth having: a specification full of shell examples.
    #[test]
    fn a_hash_inside_a_fenced_block_is_not_a_heading() {
        let outline = outline_of("# Real\n```bash\n# not a heading\n```\n## Also real");

        let titles: Vec<&str> = outline.iter().map(|itm| itm.title.as_str()).collect();
        assert_eq!(titles, vec!["Real", "Also real"]);
    }

    #[test]
    fn a_tilde_fence_closes_only_on_a_tilde_fence() {
        let outline = outline_of("~~~\n# hidden\n```\n# still hidden\n~~~\n# out");

        let titles: Vec<&str> = outline.iter().map(|itm| itm.title.as_str()).collect();
        assert_eq!(titles, vec!["out"]);
    }

    /// A longer closing fence closes a shorter opening one; a shorter one does not close a longer one.
    #[test]
    fn fence_lengths_are_respected() {
        let outline = outline_of("````\n```\n# hidden\n````\n# out");

        let titles: Vec<&str> = outline.iter().map(|itm| itm.title.as_str()).collect();
        assert_eq!(titles, vec!["out"]);
    }

    #[test]
    fn what_is_not_a_heading_is_not_one() {
        let outline = outline_of("#hashtag\n####### seven hashes\n    # indented code\ntext");

        assert!(outline.is_empty(), "{outline:?}");
    }

    #[test]
    fn a_closing_hash_run_is_decoration_but_a_sharp_in_a_name_is_not() {
        assert_eq!(outline_of("## Two ##")[0].title, "Two");
        assert_eq!(outline_of("## C#")[0].title, "C#");
    }

    /// Indentation is COLUMNS, not bytes. Measured in bytes, two non-breaking spaces are four and the
    /// heading disappears from the outline — which an agent can only read as the section not existing.
    #[test]
    fn a_heading_indented_with_multibyte_whitespace_is_still_a_heading() {
        let outline = outline_of("\u{a0}\u{a0}# Nbsp heading");

        assert_eq!(outline.len(), 1, "{outline:?}");
        assert_eq!(outline[0].title, "Nbsp heading");
    }

    /// The section size counts the terminators the document actually has.
    #[test]
    fn a_section_size_counts_real_bytes_including_crlf() {
        let outline = outline_of("# One\r\nbody\r\n");

        // "# One\r\nbody\r\n" is 13 bytes, and the section is the whole document.
        assert_eq!(outline[0].size, 13);
    }

    #[test]
    fn a_document_with_no_headings_has_no_outline() {
        assert!(outline_of("just some prose\nand more of it").is_empty());
    }

    // ------------------------------------------------------------------------------------- diffing

    #[test]
    fn a_diff_shows_the_change_and_the_two_labels() {
        let (diff, truncated) = unified_diff("a\nb\nc\n", "a\nB\nc\n", "old", "new", 1);

        assert!(!truncated);
        assert!(diff.contains("--- old"), "{diff}");
        assert!(diff.contains("+++ new"), "{diff}");
        assert!(diff.contains("-b"), "{diff}");
        assert!(diff.contains("+B"), "{diff}");
    }

    #[test]
    fn two_identical_texts_diff_to_nothing() {
        let (diff, _) = unified_diff("same\n", "same\n", "a", "b", 3);

        assert!(
            !diff.contains("@@"),
            "there should be no hunk at all: {diff}"
        );
    }

    /// An enormous diff is cut rather than returned — and cut at a character boundary.
    #[test]
    fn an_enormous_diff_is_cut_and_says_so() {
        let before = "é\n".repeat(200_000);
        let after = "è\n".repeat(200_000);

        let (diff, truncated) = unified_diff(&before, &after, "a", "b", 3);

        assert!(truncated);
        assert!(diff.len() <= MAX_DIFF_BYTES);
    }
}
