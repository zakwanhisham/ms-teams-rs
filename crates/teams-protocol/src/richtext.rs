//! Teams message HTML → a small render-ready [`RichText`] AST.
//!
//! Bodies with `messagetype = "RichText/Html"` carry compact HTML (see the wire dialect
//! note in `docs/endpoints.md`): `<p>` runs with `margin-left` indentation, mention
//! `<span>`s cross-referencing `properties.mentions` by `itemid`, `animated-emoticon`
//! spans, links, `strong`, lists and `table > tbody > tr > td`. Teams routinely
//! mis-nests (`p` containing `ul`, `li` directly inside `li`), so the parse goes through
//! html5ever — the HTML5 spec tree-construction — which normalizes that into a
//! well-formed tree before we walk it. Unknown tags degrade transparently: their *text*
//! content is kept, because rendering must never lose message content it does not model.
//!
//! [`parse`] builds the bare AST, [`parse_with_mentions`] additionally resolves mention
//! spans against the message's `properties.mentions`, and [`RichText::to_plain_text`]
//! exports a readable terminal rendering. Not seen in live captures but implemented for
//! the standard dialect (labelled expected-not-yet-seen): `em`/`i`, `u`, `s`/`del`,
//! inline `code`, `pre` blocks, `blockquote`, headings, thematic breaks.
//!
//! # Example
//!
//! ```
//! use teams_protocol::richtext::{parse, Block, Inline};
//!
//! let text = parse("<p>hello <strong>world</strong></p>");
//! assert_eq!(text.to_plain_text(), "hello world");
//! assert!(matches!(
//!     text.blocks.as_slice(),
//!     [Block::Paragraph { inlines, .. }] if matches!(inlines.as_slice(), [Inline::Text(_), Inline::Bold(_)])
//! ));
//! ```

use html5ever::tendril::TendrilSink;
use html5ever::{ParseOpts, QualName, local_name, ns, parse_fragment};
use markup5ever_rcdom::{Handle, NodeData, RcDom};
use serde::Serialize;

use crate::Mention;

/// Render-ready representation of one message body.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct RichText {
    /// Top-level flow of the message: paragraphs, lists, tables, quotes, code blocks.
    pub blocks: Vec<Block>,
}

/// One top-level element of a message body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum Block {
    /// A `<p>` run; Teams communicates list indentation via `margin-left` on it
    /// (measured live: `margin-left:40px`). `inlines` may be empty for a visual break.
    Paragraph {
        /// `margin-left` in px, if the wire declared one.
        indent_px: Option<u32>,
        /// Inline content.
        inlines: Vec<Inline>,
    },
    /// A `<ul>`/`<ol>`; nested lists ride as [`Block::List`] inside their item.
    List {
        /// `true` for `<ol>`.
        ordered: bool,
        /// One entry per `<li>` (parser normalizes misnested `li` to siblings).
        items: Vec<ListItem>,
    },
    /// A `<table>` (live: `table > tbody > tr > td`).
    Table {
        /// One per `<tr>`.
        rows: Vec<TableRow>,
    },
    /// A `<blockquote>`; contents as their own block flow.
    Quote {
        /// Inner blocks, each line rendered with a quote marker in plain text.
        blocks: Vec<Block>,
    },
    /// A `<pre>` (not yet seen live; standard dialect).
    Code {
        /// Verbatim text content (entities already decoded by the parser).
        text: String,
        /// `language-…` from a nested `<code>`, when present.
        language: Option<String>,
    },
}

/// One `<li>` of a list.
///
/// Plain items become one [`Block::Paragraph`] in `blocks`; block children that stay
/// inside the item (`li > p`, `li > ul` — both seen live) land here verbatim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ListItem {
    /// The item's block flow.
    pub blocks: Vec<Block>,
}

/// One `<tr>` of a table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TableRow {
    /// `<th>`/`<td>` cells, in order.
    pub cells: Vec<TableCell>,
}

/// One table cell.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TableCell {
    /// `true` for `<th>` (not seen live).
    pub header: bool,
    /// Inline content; block children degrade to text separated by line breaks.
    pub inlines: Vec<Inline>,
}

/// One styled / annotated run inside a block.
///
/// Style variants wrap their contents (nesting preserved: `<strong><em>` turns into
/// `Bold([Italic([Text(…)])])`), so there is no stacked state to reconcile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum Inline {
    /// Verbatim text (entities already decoded by the parser).
    Text(String),
    /// `<strong>`/`<b>` (bold is the live one).
    Bold(Vec<Inline>),
    /// `<em>`/`<i>` (expected, not yet seen live).
    Italic(Vec<Inline>),
    /// `<u>`/`<ins>` (expected, not yet seen live).
    Underline(Vec<Inline>),
    /// `<s>`/`<del>`/`<strike>` (expected, not yet seen live).
    Strike(Vec<Inline>),
    /// Inline `<code>` outside a `<pre>` (expected, not yet seen live).
    Code(String),
    /// `<a href>`; Teams link targets fire `target=_blank rel="noreferrer noopener"`.
    Link {
        /// The `href` value as supplied.
        href: String,
        /// Link text (may wrap other styled runs).
        children: Vec<Inline>,
    },
    /// `<span itemscope itemtype="…schema.skype.com/" itemid="$N">` — the live mention
    /// dialect: `N` indexes `properties.mentions`, which carries the MRI.
    Mention {
        /// The span's `itemid` (the index into `properties.mentions`).
        item_id: u64,
        /// Text the span rendered (usually the display name).
        text: String,
        /// `display_name` from `properties.mentions`, when [`parse_with_mentions`]
        /// cross-referenced the two.
        resolved: Option<String>,
    },
    /// `<span class="animated-emoticon…" type="(smile)"><img …></span>` — the live
    /// emoticon dialect; `name` is the Teams emoticon name (e.g. `smile`, `1f389_partypopper`).
    Emoticon {
        /// Emoticon name from the span's `type`/child `img` `itemid`.
        name: String,
    },
    /// A bare `<img>` (inline images and file-card art); emoticon images are consumed
    /// by [`Inline::Emoticon`].
    Image {
        /// The `src` attribute.
        src: String,
        /// The `alt` attribute, when supplied.
        alt: Option<String>,
    },
    /// `<br>`.
    LineBreak,
}

/// Parse a message body into its AST.
///
/// Unknown or unmodelled tags (Teams ships some, e.g. `app-preview-card` spans) degrade
/// transparently: their content is kept as-is. Mentions are not cross-referenced;
/// use [`parse_with_mentions`] where `properties.mentions` is available.
#[must_use]
pub fn parse(html: &str) -> RichText {
    parse_inner(html, None)
}

/// Like [`parse`], additionally filling [`Inline::Mention::resolved`] by cross-referencing
/// each span's `itemid` against `properties.mentions` (see [`crate::Mention::item_id`]).
#[must_use]
pub fn parse_with_mentions(html: &str, mentions: &[Mention]) -> RichText {
    parse_inner(html, Some(mentions))
}

/// Parse the fragment with a `div` context (the generic block container), then walk it.
fn parse_inner(html: &str, mentions: Option<&[Mention]>) -> RichText {
    let parser = parse_fragment(
        RcDom::default(),
        ParseOpts::default(),
        // Any block container works as fragment context; `div` keeps list/table
        // tree-construction (the "in body" rules) active without extra semantics.
        QualName::new(None, ns!(html), local_name!("div")),
        Vec::new(),
        false,
    );
    let dom = parser.one(html);
    let mut rich_text = RichText::default();
    blocks_from_children(&fragment_root(&dom), mentions, &mut rich_text.blocks);
    rich_text
}

/// The handle whose children are the parsed fragment nodes.
///
/// html5ever's fragment parse appends a synthetic `<html>` root under the document
/// (`tree_builder::create_root`); the real fragment nodes are its children. The check
/// also tolerates nodes landing directly under the document.
fn fragment_root(dom: &RcDom) -> Handle {
    let children = dom.document.children.borrow();
    if let Some(only) = children.first()
        && let NodeData::Element { name, .. } = &only.data
        && *name.local == *local_name!("html")
    {
        return only.clone();
    }
    drop(children);
    dom.document.clone()
}

/// Walk a *block* container's children into out — the transport for everything outside
/// `<p>`/`<li>`/cells: the fragment root, `<div>`, `<blockquote>`, `<li>`.
///
/// Inline runs that appear between block-level elements (bare text, `<strong>`, `<span>`,
/// `<a>` at the root…) collect into an implied paragraph; empty paragraph elements are
/// dropped so a whitespace-only body parses to `is_empty() == true`.
fn blocks_from_children(container: &Handle, mentions: Option<&[Mention]>, out: &mut Vec<Block>) {
    let children = container.children.borrow();
    // Inline runs that appear between block-level elements (bare text, `<strong>`,
    // `<span>`, `<a>` at the root…) collect into an implied paragraph.
    let mut pending: Vec<Inline> = Vec::new();

    for node in children.iter() {
        match &node.data {
            NodeData::Text { contents } => {
                // Whitespace between block tags is layout, not content.
                if contents.borrow().trim().is_empty() {
                    continue;
                }
                pending.push(Inline::Text(contents.borrow().to_string()));
            }
            NodeData::Element { name, attrs, .. } => {
                let attrs = attrs.borrow();
                match name.local.as_ref() {
                    "p" => {
                        flush_pending(&mut pending, out);
                        let inlines = inlines_of_children(node, mentions);
                        let indent_px = margin_left(&attrs);
                        // Empty paragraphs carry nothing; keep ones with indentation
                        // (visual spacing) and drop the rest.
                        if !inlines.is_empty() || indent_px.is_some() {
                            out.push(Block::Paragraph { indent_px, inlines });
                        }
                    }
                    "ul" | "ol" => {
                        flush_pending(&mut pending, out);
                        out.push(list_block(node, name.local.as_ref() == "ol", mentions));
                    }
                    "table" => {
                        flush_pending(&mut pending, out);
                        out.push(Block::Table { rows: table_rows(node, mentions) });
                    }
                    "blockquote" => {
                        flush_pending(&mut pending, out);
                        let mut blocks = Vec::new();
                        blocks_from_children(node, mentions, &mut blocks);
                        out.push(Block::Quote { blocks });
                    }
                    "pre" => {
                        flush_pending(&mut pending, out);
                        out.push(Block::Code {
                            text: text_content(node),
                            language: code_language(node),
                        });
                    }
                    // Teams does not send headings; treat them as plain emphasis-free
                    // paragraphs rather than dropping them.
                    "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                        flush_pending(&mut pending, out);
                        out.push(Block::Paragraph {
                            indent_px: None,
                            inlines: inlines_of_children(node, mentions),
                        });
                    }
                    // Generic block containers: recurse (their text becomes paragraphs).
                    "div" | "section" | "article" | "main" => {
                        flush_pending(&mut pending, out);
                        blocks_from_children(node, mentions, out);
                    }
                    // A bare `li` outside a list (malformed markup) is still a list item;
                    // wrap it in a one-item list so the bullet semantics survive.
                    "li" => {
                        flush_pending(&mut pending, out);
                        out.push(Block::List {
                            ordered: false,
                            items: vec![list_item(node, mentions)],
                        });
                    }
                    // Thematic break: purely decorative rule, dropped deliberately
                    // (documented degradation; not seen live).
                    "hr" => flush_pending(&mut pending, out),
                    _ => pending.extend(inlines_of_children(node, mentions)),
                }
            }
            _ => {} // comments, doctypes, processing instructions
        }
    }
    drop(children);
    flush_pending(&mut pending, out);
}

/// Inline runs that reached the block walker land in their own implied paragraph.
fn flush_pending(pending: &mut Vec<Inline>, out: &mut Vec<Block>) {
    if pending.is_empty() {
        return;
    }
    let inlines = std::mem::take(pending);
    out.push(Block::Paragraph { indent_px: None, inlines });
}

fn list_block(list: &Handle, ordered: bool, mentions: Option<&[Mention]>) -> Block {
    Block::List { ordered, items: list_items(list, mentions) }
}

/// Collect the `li` items of a list element (shared by the block and inline paths).
fn list_items(list: &Handle, mentions: Option<&[Mention]>) -> Vec<ListItem> {
    let mut items = Vec::new();
    let children = list.children.borrow();
    for node in children.iter() {
        // Non-`li` children (whitespace, stray markup) are outside the list's model
        // and are ignored.
        if let NodeData::Element { name, .. } = &node.data
            && name.local.as_ref() == "li"
        {
            items.push(list_item(node, mentions));
        }
    }
    drop(children);
    items
}

fn list_item(item: &Handle, mentions: Option<&[Mention]>) -> ListItem {
    // `<li>` content is a block flow in its own right: inline runs become paragraphs,
    // `li > p` / `li > ul` (both observed live) land in `blocks`.
    let mut blocks = Vec::new();
    blocks_from_children(item, mentions, &mut blocks);
    ListItem { blocks }
}

fn table_rows(table: &Handle, mentions: Option<&[Mention]>) -> Vec<TableRow> {
    let mut rows = Vec::new();
    let children = table.children.borrow();
    for node in children.iter() {
        if let NodeData::Element { name, .. } = &node.data {
            match name.local.as_ref() {
                // Direct `<tr>` children (tbody-less tables) work too.
                "tr" => rows.push(table_row(node, mentions)),
                "tbody" | "thead" | "tfoot" => {
                    let section = node.children.borrow();
                    for row in section.iter() {
                        if let NodeData::Element { name, .. } = &row.data
                            && name.local.as_ref() == "tr"
                        {
                            rows.push(table_row(row, mentions));
                        }
                    }
                }
                _ => {}
            }
        }
    }
    drop(children);
    rows
}

fn table_row(row: &Handle, mentions: Option<&[Mention]>) -> TableRow {
    let mut cells = Vec::new();
    let children = row.children.borrow();
    for node in children.iter() {
        // Unknown children (text, stray markup) are outside the row's model and are
        // ignored.
        if let NodeData::Element { name, .. } = &node.data
            && matches!(name.local.as_ref(), "td" | "th")
        {
            cells.push(TableCell {
                header: name.local.as_ref() == "th",
                inlines: inlines_of_children(node, mentions),
            });
        }
    }
    drop(children);
    TableRow { cells }
}

/// Walk a block's children as *inline* content (the `<p>`/`<li>`/cell body).
///
/// Block-ish elements reached here (e.g. `li > p`, seen live) keep their separation by
/// surfacing as [`Inline::LineBreak`] boundaries instead of merging into one run.
fn inlines_of_children(block: &Handle, mentions: Option<&[Mention]>) -> Vec<Inline> {
    let children = block.children.borrow();
    let mut inlines: Vec<Inline> = Vec::new();
    // Set after block-ish content inside an inline run ("block boundary pending") so a
    // following run gets a `LineBreak` separator, but a preceding one does not.
    let mut boundary = false;

    for node in children.iter() {
        match &node.data {
            NodeData::Text { contents } => {
                if boundary {
                    inlines.push(Inline::LineBreak);
                    boundary = false;
                }
                // Non-whitespace inline text is kept verbatim (entities are decoded by
                // the parser); whitespace-only runs between tags are dropped.
                if !contents.borrow().trim().is_empty() {
                    inlines.push(Inline::Text(contents.borrow().to_string()));
                }
            }
            NodeData::Element { name, attrs, .. } => {
                let name_local = name.local.as_ref();
                let attrs = attrs.borrow();
                match name_local {
                    "p" | "div" | "section" | "article" | "main" | "h1" | "h2" | "h3" | "h4"
                    | "h5" | "h6" | "blockquote" => {
                        if boundary || !inlines.is_empty() {
                            inlines.push(Inline::LineBreak);
                        }
                        inlines.extend(inlines_of_children(node, mentions));
                        boundary = true;
                    }
                    "ul" | "ol" => {
                        // A list inside an inline run flattens into one line per item
                        // (structure degraded; only observed in malformed messages).
                        flatten_list_inline(node, mentions, &mut boundary, &mut inlines);
                    }
                    "table" => {
                        flatten_table_inline(node, mentions, &mut boundary, &mut inlines);
                    }
                    "pre" => {
                        if boundary || !inlines.is_empty() {
                            inlines.push(Inline::LineBreak);
                        }
                        inlines.push(Inline::Code(text_content(node)));
                        boundary = true;
                    }
                    "br" => inlines.push(Inline::LineBreak),
                    "img" => {
                        let image = inline_image(&attrs);
                        if let Some(image) = image {
                            inlines.push(image);
                        }
                    }
                    "strong" | "b" => {
                        wrap(&mut inlines, Inline::Bold, node, mentions);
                    }
                    "em" | "i" => wrap(&mut inlines, Inline::Italic, node, mentions),
                    "u" | "ins" => wrap(&mut inlines, Inline::Underline, node, mentions),
                    "s" | "del" | "strike" => wrap(&mut inlines, Inline::Strike, node, mentions),
                    "code" => inlines.push(Inline::Code(text_content(node))),
                    "a" => {
                        let children = inlines_of_children(node, mentions);
                        if let Some(href) = attr(&attrs, "href") {
                            inlines.push(Inline::Link { href: href.to_owned(), children });
                        } else {
                            inlines.extend(children);
                        }
                    }
                    "span" => match span_kind(&attrs) {
                        SpanKind::Mention { item_id } => {
                            if boundary {
                                inlines.push(Inline::LineBreak);
                                boundary = false;
                            }
                            inlines.push(Inline::Mention {
                                item_id,
                                text: text_content(node),
                                resolved: mentions.and_then(|all| {
                                    all.iter()
                                        .find(|w| w.item_id == Some(item_id))
                                        .and_then(|w| w.display_name.clone())
                                }),
                            });
                        }
                        SpanKind::Emoticon { name } => {
                            inlines.push(Inline::Emoticon { name });
                        }
                        // Unknown `span` dialects (style wrappers, `app-preview-card`…)
                        // render their content untouched.
                        SpanKind::Transparent => {
                            inlines.extend(inlines_of_children(node, mentions));
                        }
                    },
                    _ => {
                        inlines.extend(inlines_of_children(node, mentions));
                    }
                }
            }
            _ => {} // comments et al.
        }
    }
    drop(children);
    inlines
}

/// Wrap `node`'s children into `build` and append to `out` (empty results are skipped).
fn wrap(
    out: &mut Vec<Inline>,
    build: fn(Vec<Inline>) -> Inline,
    node: &Handle,
    mentions: Option<&[Mention]>,
) {
    let children = inlines_of_children(node, mentions);
    if !children.is_empty() {
        out.push(build(children));
    }
}

/// Flatten a list that was reached inside an inline run: one line per item
/// (structure degraded; only observed in malformed messages).
fn flatten_list_inline(
    list: &Handle,
    mentions: Option<&[Mention]>,
    boundary: &mut bool,
    out: &mut Vec<Inline>,
) {
    let items = list_items(list, mentions);
    if items.is_empty() {
        return;
    }
    // The list is block-level inline content: break before it.
    if *boundary || !out.is_empty() {
        out.push(Inline::LineBreak);
    }
    let mut first = true;
    for item in items {
        if !first {
            out.push(Inline::LineBreak);
        }
        first = false;
        for block in &item.blocks {
            if let Block::Paragraph { inlines: run, .. } = block {
                out.extend(run.iter().cloned());
            }
        }
    }
    // Separate the list from whatever follows.
    *boundary = true;
}

/// Same treatment for a table reached inside an inline run.
fn flatten_table_inline(
    table: &Handle,
    mentions: Option<&[Mention]>,
    boundary: &mut bool,
    out: &mut Vec<Inline>,
) {
    let rows = table_rows(table, mentions);
    if rows.is_empty() {
        return;
    }
    if *boundary || !out.is_empty() {
        out.push(Inline::LineBreak);
    }
    let mut first = true;
    for row in rows {
        if !first {
            out.push(Inline::LineBreak);
        }
        first = false;
        for cell in &row.cells {
            out.extend(cell.inlines.iter().cloned());
        }
    }
    *boundary = true;
}

/// What kind of `<span>` this is, from the live dialect rules.
enum SpanKind {
    /// `itemid="N"` (numeric): `properties.mentions[N]`.
    Mention { item_id: u64 },
    /// `animated-emoticon` span (`type="(smile)"`, `itemid` on the child `img`).
    Emoticon { name: String },
    /// Anything else: pass children through.
    Transparent,
}

fn span_kind(attrs: &[html5ever::Attribute]) -> SpanKind {
    // Mentions: `itemid` parses as the numeric mention index.
    if let Some(item_id) = attr(attrs, "itemid").and_then(|raw| raw.parse::<u64>().ok()) {
        return SpanKind::Mention { item_id };
    }
    // Emoticons: `type="(smile)"` or the `animated-emoticon-*` class.
    let type_name = attr(attrs, "type")
        .map(str::trim)
        .filter(|raw| raw.starts_with('(') && raw.ends_with(')'))
        .map(|raw| raw[1..raw.len() - 1].to_owned());
    let is_emoticon = type_name.is_some()
        || attr(attrs, "class").is_some_and(|class| class.contains("animated-emoticon"));
    if is_emoticon {
        let name = type_name.unwrap_or_else(|| {
            // Fallback: the class tail (`animated-emoticon-20-1f389_partypopper`
            // matches the child `img`'s `itemid` on the wire).
            attr(attrs, "class")
                .map(|class| class.rsplit('-').next().unwrap_or(class).to_owned())
                .unwrap_or_default()
        });
        return SpanKind::Emoticon { name };
    }
    SpanKind::Transparent
}

/// A bare (non-emoticon) `<img>`; `src`-less images are skipped as unrenderable.
fn inline_image(attrs: &[html5ever::Attribute]) -> Option<Inline> {
    let src = attr(attrs, "src").map(str::to_owned)?;
    Some(Inline::Image { src, alt: attr(attrs, "alt").map(str::to_owned) })
}

fn attr<'a>(attrs: &'a [html5ever::Attribute], name: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|candidate| candidate.name.local.as_ref() == name)
        .map(|candidate| candidate.value.as_ref())
}

/// `style="margin-left:40px"` → `40` (the live list-indentation dialect).
fn margin_left(attrs: &[html5ever::Attribute]) -> Option<u32> {
    let style = attr(attrs, "style")?;
    let raw = style.split(';').find(|decl| {
        let decl = decl.trim();
        decl.starts_with("margin-left")
    })?;
    let value = raw.split(':').nth(1)?;
    let digits: String = value.trim_start().chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// Concatenated text of everything under `node` (used for `<pre>`/`<code>` blocks).
fn text_content(node: &Handle) -> String {
    let mut out = String::new();
    let mut stack = vec![node.clone()];
    while let Some(handle) = stack.pop() {
        for child in handle.children.borrow().iter().rev() {
            match &child.data {
                NodeData::Text { contents } => out.push_str(&contents.borrow()),
                NodeData::Element { .. } => stack.push(child.clone()),
                _ => {}
            }
        }
    }
    out
}

/// `language-…` from a `<code class="language-…">` nested in the `<pre>`.
fn code_language(pre: &Handle) -> Option<String> {
    let children = pre.children.borrow();
    for child in children.iter() {
        if let NodeData::Element { name, attrs, .. } = &child.data
            && name.local.as_ref() == "code"
            && let Some(class) = attr(&attrs.borrow(), "class")
        {
            let language = class
                .split_whitespace()
                .find_map(|entry| entry.strip_prefix("language-").map(str::to_owned));
            if language.is_some() {
                return language;
            }
        }
    }
    None
}

impl RichText {
    /// `true` when nothing was modelled (empty or whitespace-only input).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    /// Readable plain-text export: one block per line, list items bulleted (nested
    /// lists indented), table cells joined with `|`, quotes prefixed with `>`,
    /// code verbatim. Styled runs flatten to their text; mentions render as the
    /// span text (cross-referenced name when [`parse_with_mentions`] had one);
    /// emoticons leave nothing behind in text form.
    #[must_use]
    pub fn to_plain_text(&self) -> String {
        let mut lines = Vec::new();
        for block in &self.blocks {
            block_plain(block, &Layout { indent: 0, quoted: false }, &mut None, &mut lines);
        }
        let mut text = lines.join("\n");
        // No trailing newline for a chat-message export.
        while text.ends_with('\n') {
            text.pop();
        }
        text
    }
}

/// Layout context for the plain-text export.
struct Layout {
    indent: usize,
    quoted: bool,
}

impl Layout {
    fn margin(&self) -> String {
        "  ".repeat(self.indent)
    }

    fn child(&self, extra_indent: bool) -> Layout {
        Layout { indent: self.indent + usize::from(extra_indent), quoted: false }
    }
}

/// Write one block's lines into `lines`. `bullet` is an optional prefix that is
/// consumed by this block's first emitted line (used for list items).
fn block_plain(
    block: &Block,
    layout: &Layout,
    bullet: &mut Option<String>,
    lines: &mut Vec<String>,
) {
    match block {
        Block::Paragraph { inlines, .. } => {
            for piece in inline_plain(inlines).split('\n') {
                let piece = piece.trim();
                if !piece.is_empty() {
                    emit(lines, layout, bullet, piece);
                }
            }
        }
        Block::List { ordered, items } => {
            for (index, item) in items.iter().enumerate() {
                let prefix = if *ordered { format!("{}. ", index + 1) } else { "• ".to_owned() };
                let mut bullet = Some(prefix);
                for block in &item.blocks {
                    // Item content lines up with its bullet; nested lists indent one
                    // step further.
                    let item_layout = match block {
                        Block::List { .. } => layout.child(true),
                        _ => Layout { indent: layout.indent, quoted: layout.quoted },
                    };
                    block_plain(block, &item_layout, &mut bullet, lines);
                }
            }
        }
        Block::Table { rows } => {
            for row in rows {
                let cells: Vec<String> = row
                    .cells
                    .iter()
                    .map(|cell| inline_plain(&cell.inlines).trim().to_owned())
                    .filter(|text| !text.is_empty())
                    .collect();
                if !cells.is_empty() {
                    emit(lines, layout, bullet, cells.join(" | ").as_str());
                }
            }
        }
        Block::Quote { blocks } => {
            let quoted = Layout { indent: layout.indent, quoted: true };
            for block in blocks {
                block_plain(block, &quoted, bullet, lines);
            }
        }
        Block::Code { text, .. } => {
            for line in text.lines() {
                emit(lines, layout, bullet, line);
            }
        }
    }
}

/// Append one content line, applying the block's indent + quote marker and consuming
/// the bullet on its first line.
fn emit(lines: &mut Vec<String>, layout: &Layout, bullet: &mut Option<String>, content: &str) {
    let marker = if layout.quoted { "> " } else { "" };
    match bullet.take() {
        Some(prefix) => lines.push(format!("{}{}{}{}", layout.margin(), prefix, marker, content)),
        None => lines.push(format!("{}{}{}", layout.margin(), marker, content)),
    }
}

/// Flatten inline runs to text (newlines between fused lines).
fn inline_plain(inlines: &[Inline]) -> String {
    let mut out = String::new();
    for inline in inlines {
        match inline {
            Inline::Text(text) | Inline::Code(text) => out.push_str(text),
            Inline::Bold(children)
            | Inline::Italic(children)
            | Inline::Underline(children)
            | Inline::Strike(children) => {
                let inner = inline_plain(children);
                out.push_str(&inner);
            }
            Inline::Link { href: _, children } => out.push_str(&inline_plain(children)),
            Inline::Mention { item_id: _, text, resolved } => {
                let display = match (text.as_str(), resolved.as_deref()) {
                    ("", Some(name)) => name,
                    _ => text,
                };
                out.push_str(display);
            }
            Inline::Emoticon { name: _ } => {}
            Inline::Image { src: _, alt } => out.push_str(alt.as_deref().unwrap_or("")),
            Inline::LineBreak => out.push('\n'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one_paragraph(html: &str) -> Vec<Inline> {
        let mut text = parse(html);
        assert_eq!(text.blocks.len(), 1, "expected one block, got {:?}", text.blocks);
        match text.blocks.pop().expect("checked length") {
            Block::Paragraph { inlines, .. } => inlines,
            block => panic!("expected a paragraph, got {block:?}"),
        }
    }

    #[test]
    fn paragraphs_become_text_runs() {
        let inlines = one_paragraph("<p>the <strong>bold</strong> word</p>");
        assert_eq!(
            inlines,
            vec![
                Inline::Text("the ".into()),
                Inline::Bold(vec![Inline::Text("bold".into())]),
                Inline::Text(" word".into()),
            ]
        );
    }

    #[test]
    fn entities_are_decoded_by_the_parser() {
        let inlines = one_paragraph("<p>a &amp; b &lt; tags &quot;quoted&quot;</p>");
        assert_eq!(inlines, vec![Inline::Text("a & b < tags \"quoted\"".into())]);
    }

    #[test]
    fn adjacent_paragraphs_never_merge() {
        // The parser closes `<p>` automatically — Teams relies on this.
        let text = parse("<p>first<p>second");
        assert_eq!(text.blocks.len(), 2, "{text:?}");
    }

    #[test]
    fn empty_and_whitespace_input_model_nothing() {
        for html in ["", "  \n  ", "<p>  </p>", "<br>"] {
            assert!(parse(html).is_empty(), "{html:?} should be empty");
        }
    }

    #[test]
    fn links_keep_href_and_children() {
        let inlines = one_paragraph(
            "<p>see <a href=\"https://example.test/docs\" target=\"_blank\" \
             rel=\"noreferrer noopener\" title=\"Docs\">the <strong>docs</strong></a></p>",
        );
        assert_eq!(
            inlines,
            vec![
                Inline::Text("see ".into()),
                Inline::Link {
                    href: "https://example.test/docs".into(),
                    children: vec![
                        Inline::Text("the ".into()),
                        Inline::Bold(vec![Inline::Text("docs".into())]),
                    ],
                },
            ]
        );
    }

    #[test]
    fn attributeless_anchors_degrade_to_their_text() {
        let inlines = one_paragraph("<p><a>orphan</a></p>");
        assert_eq!(inlines, vec![Inline::Text("orphan".into())]);
    }

    #[test]
    fn mention_spans_cross_reference_properties() {
        let html = "<p>ping <span itemscope itemtype=\"http://schema.skype.com/\" \
                    itemid=\"1\">@Bex</span></p>";
        let modelled = parse(html);
        let Block::Paragraph { inlines, .. } = &modelled.blocks[0] else { panic!() };
        assert_eq!(inlines[1], Inline::Mention { item_id: 1, text: "@Bex".into(), resolved: None });

        // With the message's `properties.mentions` the name resolves.
        let mentioned = Mention {
            schema: Some("http://schema.skype.com/Mention".into()),
            item_id: Some(1),
            mri: Some("8:orgid:00000000-0000-0000-0000-000000000000".into()),
            mention_type: Some("person".into()),
            display_name: Some("Bex".into()),
            extra: serde_json::Map::new(),
        };
        let resolved = parse_with_mentions(html, &[mentioned]);
        let Block::Paragraph { inlines, .. } = &resolved.blocks[0] else { panic!() };
        assert_eq!(
            inlines[1],
            Inline::Mention { item_id: 1, text: "@Bex".into(), resolved: Some("Bex".into()) }
        );
        assert_eq!(resolved.to_plain_text(), "ping @Bex");
    }

    #[test]
    fn non_numeric_itemid_spans_degrade_to_their_content() {
        // `app-preview-card` spans (seen live) must not become mentions.
        let inlines = one_paragraph(
            "<span itemscope itemtype=\"http://schema.skype.com/\" \
             itemid=\"app-preview-card-ps012\"><p>card body</p></span>",
        );
        assert_eq!(inlines, vec![Inline::Text("card body".into())]);
    }

    #[test]
    fn emoticon_spans_use_the_type_name() {
        let inlines = one_paragraph(
            "<p>hi<span class=\"animated-emoticon-20-smile\" type=\"(smile)\" \
             title=\"Smile\" itemscope><img style=\"width:20px; height:20px\" \
             alt=\"Smile\" src=\"https://cdn.invalid/smile.gif\" itemid=\"smile\" \
             itemscope itemtype=\"http://schema.skype.com/\"/></span></p>",
        );
        assert_eq!(
            inlines,
            vec![Inline::Text("hi".into()), Inline::Emoticon { name: "smile".into() },]
        );
    }

    #[test]
    fn emoticon_without_type_falls_back_to_the_class_tail() {
        let inlines = one_paragraph(
            "<p><span class=\"animated-emoticon-20-1f389_partypopper\" \
             title=\"Party\" itemscope><img src=\"https://cdn.invalid/party.gif\" \
             alt=\"Party\"/></span></p>",
        );
        assert_eq!(inlines, vec![Inline::Emoticon { name: "1f389_partypopper".into() },]);
    }

    #[test]
    fn bare_images_keep_src_and_alt() {
        let inlines =
            one_paragraph("<p><img src=\"https://cdn.invalid/pixel.png\" alt=\"diagram\"></p>");
        assert_eq!(
            inlines,
            vec![Inline::Image {
                src: "https://cdn.invalid/pixel.png".into(),
                alt: Some("diagram".into()),
            }]
        );
    }

    #[test]
    fn line_breaks_split_paragraph_text() {
        let text = parse("<p>one<br>two</p>");
        assert_eq!(text.to_plain_text(), "one\ntwo");
    }

    #[test]
    fn misnested_list_values_stay_structured() {
        // p containing ul (seen live): the parser closes the paragraph first.
        let text = parse("<p>a<ul><li>one</li><li>two</li></ul>");
        assert_eq!(
            text.blocks,
            vec![
                Block::Paragraph { indent_px: None, inlines: vec![Inline::Text("a".into())] },
                Block::List {
                    ordered: false,
                    items: vec![
                        ListItem { blocks: vec![paragraph(["one"])] },
                        ListItem { blocks: vec![paragraph(["two"])] },
                    ],
                },
            ]
        );
    }

    #[test]
    fn li_inside_li_normalizes_to_siblings() {
        let text = parse("<ul><li>first<li>second</ul>");
        let Block::List { ordered, items } = &text.blocks[0] else { panic!("{text:?}") };
        assert!(!ordered);
        assert_eq!(items.len(), 2, "misnested li must not collapse into one item");
    }

    #[test]
    fn nested_lists_stay_nested() {
        let text = parse("<ul><li>a<ul><li>sub</li></ul></li></ul>");
        let Block::List { items, .. } = &text.blocks[0] else { panic!("{text:?}") };
        let [item] = items.as_slice() else { panic!("{text:?}") };
        assert_eq!(
            item.blocks,
            vec![
                paragraph(["a"]),
                Block::List {
                    ordered: false,
                    items: vec![ListItem { blocks: vec![paragraph(["sub"])] }],
                },
            ]
        );
        assert_eq!(text.to_plain_text(), "• a\n  • sub");
    }

    #[test]
    fn margin_left_indentation_is_measured() {
        let text = parse("<p style=\"margin-left:40px\">indented</p>");
        assert_eq!(
            text.blocks,
            vec![Block::Paragraph {
                indent_px: Some(40),
                inlines: vec![Inline::Text("indented".into())],
            }]
        );
    }

    #[test]
    fn tables_keep_cell_order_and_header_markers() {
        let text = parse(
            "<table><tbody><tr><th>head</th><td>cell</td></tr>\
             <tr><td>a</td><td>b</td></tr></tbody></table>",
        );
        let Block::Table { rows } = &text.blocks[0] else { panic!("{text:?}") };
        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows[0].cells,
            vec![
                TableCell { header: true, inlines: vec![Inline::Text("head".into())] },
                TableCell { header: false, inlines: vec![Inline::Text("cell".into())] },
            ]
        );
        assert_eq!(text.to_plain_text(), "head | cell\na | b");
    }

    #[test]
    fn quotes_wrap_their_block_flow() {
        let text = parse("<blockquote><p>pine quote</p></blockquote>");
        assert_eq!(text.blocks, vec![Block::Quote { blocks: vec![paragraph(["pine quote"])] }]);
        assert_eq!(text.to_plain_text(), "> pine quote");
    }

    #[test]
    fn pre_blocks_capture_their_text_and_language() {
        let text = parse("<pre><code class=\"language-rust\">fn main() {}</code></pre>");
        assert_eq!(
            text.blocks,
            vec![Block::Code { text: "fn main() {}".into(), language: Some("rust".into()) }]
        );
        assert_eq!(text.to_plain_text(), "fn main() {}");
    }

    #[test]
    fn headings_arrive_as_plain_paragraphs() {
        let text = parse("<h2>note</h2>");
        assert_eq!(text.blocks, vec![paragraph(["note"])]);
    }

    #[test]
    fn unknown_tags_degrade_to_their_text() {
        assert_eq!(parse("<font>text</font>").to_plain_text(), "text");
        assert_eq!(parse("<at>Rex</at>").to_plain_text(), "Rex");
        assert_eq!(parse("<p>a<!-- html comment -->b</p>").to_plain_text(), "ab");
        assert_eq!(parse("<hr>").to_plain_text(), "");
    }

    /// Paragraph with plain-text content (test sugar).
    fn paragraph(spans: impl IntoIterator<Item = &'static str>) -> Block {
        Block::Paragraph {
            indent_px: None,
            inlines: spans.into_iter().map(|text| Inline::Text(text.into())).collect(),
        }
    }
}
