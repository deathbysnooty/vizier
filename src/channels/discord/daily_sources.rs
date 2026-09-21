//! Where the daily posts get their facts: Wikipedia articles, Wikipedia's "on
//! this day", news feeds and the article pages they link to. Everything here is
//! plain fetching and text cleaning - no model calls - so a post can only say
//! what one of these returned.

use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;

const USER_AGENT: &str = "Mozilla/5.0 (compatible; vizier-discord-bot/1.0; +https://github.com/deathbysnooty/vizier)";

/// The most article text handed to the model, in characters.
pub const ARTICLE_LIMIT: usize = 14_000;

fn client() -> Option<reqwest::Client> {
    reqwest::Client::builder().user_agent(USER_AGENT).timeout(Duration::from_secs(20)).build().ok()
}

async fn get_text(url: &str) -> Option<String> {
    let response = client()?.get(url).send().await.ok()?;
    if !response.status().is_success() {
        tracing::debug!("daily: {} answered {}", url, response.status());
        return None;
    }
    response.text().await.ok()
}

async fn get_json(url: &str) -> Option<serde_json::Value> {
    serde_json::from_str(&get_text(url).await?).ok()
}

/// Percent-encodes a query value.
pub fn enc(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{:02X}", b),
        })
        .collect()
}

// --- Wikipedia ----------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct WikiPage {
    pub title: String,
    pub url: String,
    /// Plain text, with "== Heading ==" lines between sections.
    pub text: String,
    pub image: Option<String>,
}

/// One article by its exact title (redirects followed), or `None` when it is
/// missing, a disambiguation page or too short to write from.
pub async fn wiki_page(title: &str) -> Option<WikiPage> {
    let url = format!(
        "https://en.wikipedia.org/w/api.php?action=query&format=json&formatversion=2&redirects=1\
         &prop=extracts|pageimages|info|pageprops&explaintext=1&exsectionformat=wiki&inprop=url\
         &piprop=thumbnail&pithumbsize=1000&ppprop=disambiguation&titles={}",
        enc(title)
    );
    let json = get_json(&url).await?;
    let page = json["query"]["pages"].get(0)?;
    if page.get("missing").is_some() || page["pageprops"].get("disambiguation").is_some() {
        return None;
    }
    let text = page["extract"].as_str()?.trim().to_string();
    if text.chars().count() < 800 {
        return None;
    }
    Some(WikiPage {
        title: page["title"].as_str()?.to_string(),
        url: page["fullurl"].as_str().map(str::to_string).unwrap_or_else(|| wiki_url(title)),
        text,
        image: page["thumbnail"]["source"].as_str().map(str::to_string),
    })
}

/// The best search hit's title.
pub async fn wiki_search(query: &str) -> Option<String> {
    let url = format!(
        "https://en.wikipedia.org/w/api.php?action=query&format=json&formatversion=2&list=search&srlimit=1&srsearch={}",
        enc(query)
    );
    let json = get_json(&url).await?;
    json["query"]["search"].get(0)?["title"].as_str().map(str::to_string)
}

/// The article by title, or the best search hit for it when the exact title
/// doesn't lead to an article.
pub async fn wiki_find(title: &str) -> Option<WikiPage> {
    if let Some(page) = wiki_page(title).await {
        return Some(page);
    }
    wiki_page(&wiki_search(title).await?).await
}

pub fn wiki_url(title: &str) -> String {
    format!("https://en.wikipedia.org/wiki/{}", enc(&title.replace(' ', "_")))
}

/// The lead and the sections whose headings contain one of `wanted` (with
/// their subsections), when they add up to enough to write from; otherwise the
/// whole article. Reference-type sections are always dropped. Capped at `limit`.
pub fn focus(text: &str, wanted: &[&str], limit: usize) -> String {
    const DROPPED: &[&str] = &["see also", "references", "notes", "further reading", "external links", "bibliography", "sources", "citations", "footnotes"];
    let mut lead = String::new();
    let mut picked = String::new();
    let mut whole = String::new();
    // (heading level, kept) of the section being read; None before the first heading.
    let mut current: Option<(usize, bool, bool)> = None;
    let mut top_kept = false;
    let mut top_dropped = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(level) = heading_level(trimmed) {
            let name = trimmed.trim_matches('=').trim().to_lowercase();
            if level == 2 {
                top_kept = wanted.iter().any(|w| name.contains(&w.to_lowercase()));
                top_dropped = DROPPED.iter().any(|d| name == *d);
            }
            current = Some((level, top_kept, top_dropped));
            if !top_dropped {
                whole.push_str(&format!("\n{}\n", trimmed));
                if top_kept {
                    picked.push_str(&format!("\n{}\n", trimmed));
                }
            }
            continue;
        }
        match current {
            None => {
                lead.push_str(line);
                lead.push('\n');
            }
            Some((_, kept, dropped)) => {
                if dropped {
                    continue;
                }
                whole.push_str(line);
                whole.push('\n');
                if kept {
                    picked.push_str(line);
                    picked.push('\n');
                }
            }
        }
    }
    let chosen = if picked.chars().count() >= 2500 { format!("{}\n{}", lead.trim(), picked.trim()) } else { format!("{}\n{}", lead.trim(), whole.trim()) };
    clip_chars(chosen.trim(), limit)
}

fn heading_level(line: &str) -> Option<usize> {
    if line.len() < 5 || !line.starts_with("==") || !line.ends_with("==") {
        return None;
    }
    Some(line.chars().take_while(|c| *c == '=').count())
}

/// An event from Wikipedia's "on this day" for today's date.
#[derive(Clone, Debug)]
pub struct DayEvent {
    pub year: i64,
    pub text: String,
    /// The main article the event links to.
    pub page: String,
}

/// Wikipedia's "selected" anniversaries and events for a month and day.
pub async fn on_this_day(month: u32, day: u32) -> Vec<DayEvent> {
    let url = format!("https://api.wikimedia.org/feed/v1/wikipedia/en/onthisday/all/{:02}/{:02}", month, day);
    let Some(json) = get_json(&url).await else {
        return Vec::new();
    };
    let mut events = Vec::new();
    for group in ["selected", "events"] {
        for event in json[group].as_array().into_iter().flatten() {
            let (Some(year), Some(text)) = (event["year"].as_i64(), event["text"].as_str()) else {
                continue;
            };
            // The first linked page that isn't a year is the story's article.
            let page = event["pages"].as_array().into_iter().flatten().filter_map(|p| p["title"].as_str()).find(|t| t.parse::<i64>().is_err());
            if let Some(page) = page {
                let page = page.replace('_', " ");
                if !events.iter().any(|e: &DayEvent| e.page == page) {
                    events.push(DayEvent { year, text: text.to_string(), page });
                }
            }
        }
    }
    events
}

// --- news feeds ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct FeedItem {
    pub source: &'static str,
    pub title: String,
    pub link: String,
    pub summary: String,
    /// Unix seconds; 0 when the feed didn't say.
    pub published: i64,
    pub image: Option<String>,
}

/// Every item of an RSS or Atom feed, newest as the feed ordered them.
pub async fn feed(source: &'static str, url: &str) -> Vec<FeedItem> {
    match get_text(url).await {
        Some(xml) => parse_feed(source, &xml),
        None => {
            tracing::warn!("daily: feed {} ({}) didn't load", source, url);
            Vec::new()
        }
    }
}

pub fn parse_feed(source: &'static str, xml: &str) -> Vec<FeedItem> {
    // HTML entities that XML doesn't define, written as numbers so the parser accepts them.
    let mut xml = xml.to_string();
    for (name, code) in [("nbsp", 160), ("mdash", 8212), ("ndash", 8211), ("rsquo", 8217), ("lsquo", 8216), ("ldquo", 8220), ("rdquo", 8221), ("hellip", 8230), ("eacute", 233), ("copy", 169), ("trade", 8482), ("reg", 174)] {
        xml = xml.replace(&format!("&{};", name), &format!("&#{};", code));
    }
    let options = roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() };
    let doc = match roxmltree::Document::parse_with_options(&xml, options) {
        Ok(doc) => doc,
        Err(err) => {
            tracing::warn!("daily: feed {} unreadable: {}", source, err);
            return Vec::new();
        }
    };
    let mut items = Vec::new();
    for node in doc.descendants().filter(|n| n.is_element() && matches!(n.tag_name().name(), "item" | "entry")) {
        let child = |name: &str| node.children().find(|c| c.is_element() && c.tag_name().name() == name);
        let text_of = |name: &str| child(name).map(|c| c.text().unwrap_or_default().trim().to_string()).filter(|t| !t.is_empty());
        let Some(title) = text_of("title").map(|t| html_to_text(&t)) else {
            continue;
        };
        let link = node
            .children()
            .filter(|c| c.is_element() && c.tag_name().name() == "link")
            .find_map(|c| {
                let rel = c.attribute("rel").unwrap_or("alternate");
                match c.attribute("href") {
                    Some(href) if rel == "alternate" => Some(href.to_string()),
                    Some(_) => None,
                    None => c.text().map(|t| t.trim().to_string()).filter(|t| !t.is_empty()),
                }
            })
            .or_else(|| text_of("guid").filter(|g| g.starts_with("http")));
        let Some(link) = link else {
            continue;
        };
        let body = text_of("encoded").or_else(|| text_of("content")).or_else(|| text_of("description")).or_else(|| text_of("summary")).unwrap_or_default();
        let published = text_of("pubDate")
            .and_then(|d| chrono::DateTime::parse_from_rfc2822(&d).ok())
            .or_else(|| text_of("published").or_else(|| text_of("updated")).and_then(|d| chrono::DateTime::parse_from_rfc3339(&d).ok()))
            .map(|d| d.timestamp())
            .unwrap_or(0);
        let image = node
            .descendants()
            .filter(|c| c.is_element())
            .find_map(|c| match c.tag_name().name() {
                "content" | "thumbnail" if c.attribute("url").is_some() => {
                    let medium = c.attribute("medium").or(c.attribute("type")).unwrap_or("image");
                    medium.starts_with("image").then(|| c.attribute("url").unwrap_or_default().to_string())
                }
                "enclosure" if c.attribute("type").unwrap_or_default().starts_with("image") => c.attribute("url").map(str::to_string),
                _ => None,
            })
            .or_else(|| first_img(&body));
        items.push(FeedItem { source, title, link, summary: clip_chars(&html_to_text(&body), 1500), published, image });
    }
    items
}

fn first_img(html: &str) -> Option<String> {
    static IMG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?i)<img[^>]+src=["']([^"']+)["']"#).unwrap());
    IMG.captures(html).map(|c| c[1].to_string()).filter(|u| u.starts_with("http"))
}

/// The readable paragraphs of an article page and its share image. `None` when
/// the page won't load or has too little text to write from (paywalls, apps).
pub async fn article(url: &str) -> Option<(String, Option<String>)> {
    static PARA: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?is)<p[\s>].*?</p>").unwrap());
    static OG: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"(?i)<meta[^>]+(?:property|name)=["'](?:og:image|twitter:image)["'][^>]+content=["']([^"']+)["']"#).unwrap());
    static OG_REVERSED: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"(?i)<meta[^>]+content=["']([^"']+)["'][^>]+(?:property|name)=["'](?:og:image|twitter:image)["']"#).unwrap());
    let html = get_text(url).await?;
    let image = OG.captures(&html).or_else(|| OG_REVERSED.captures(&html)).map(|c| html_to_text(&c[1])).filter(|u| u.starts_with("http"));
    let body = strip_blocks(&html);
    let paragraphs: Vec<String> = PARA
        .find_iter(&body)
        .map(|m| html_to_text(m.as_str()))
        .filter(|p| p.chars().count() >= 60 && !is_boilerplate(p))
        .collect();
    let text = paragraphs.join("\n\n");
    (text.chars().count() >= 600).then(|| (clip_chars(&text, ARTICLE_LIMIT), image))
}

/// Sign-up prompts, cookie notices and the like that sit in `<p>` tags beside the article.
fn is_boilerplate(paragraph: &str) -> bool {
    const MARKS: &[&str] = &["cookie", "subscri", "newsletter", "sign up", "email address", "your inbox", "log in", "logout", "daily email digest", "% off"];
    let lower = paragraph.to_lowercase();
    MARKS.iter().any(|m| lower.contains(m))
}

/// Drops scripts, styles, navigation and the like, whose text isn't the article.
fn strip_blocks(html: &str) -> String {
    static BLOCKS: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?is)<(script|style|noscript|nav|header|footer|aside|form|svg|figure)[\s>].*?</(script|style|noscript|nav|header|footer|aside|form|svg|figure)>").unwrap());
    BLOCKS.replace_all(html, " ").into_owned()
}

/// Tags removed, entities decoded, whitespace tidied, paragraphs kept as blank lines.
pub fn html_to_text(html: &str) -> String {
    static BREAKS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)<br\s*/?>|</p>|</li>|</h\d>|</div>").unwrap());
    static TAGS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<[^>]*>").unwrap());
    static SPACES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[ \t\u{a0}]+").unwrap());
    static BLANKS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\n\s*\n+").unwrap());
    let text = BREAKS.replace_all(html, "\n\n");
    let text = TAGS.replace_all(&text, "");
    let text = decode_entities(&text);
    let text = SPACES.replace_all(&text, " ");
    let text: String = text.lines().map(str::trim).collect::<Vec<_>>().join("\n");
    BLANKS.replace_all(text.trim(), "\n\n").into_owned()
}

fn decode_entities(text: &str) -> String {
    static ENTITY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"&(#x[0-9a-fA-F]+|#[0-9]+|[a-zA-Z]+);").unwrap());
    ENTITY
        .replace_all(text, |c: &regex::Captures| {
            let name = &c[1];
            let decoded = if let Some(hex) = name.strip_prefix("#x").or_else(|| name.strip_prefix("#X")) {
                u32::from_str_radix(hex, 16).ok().and_then(char::from_u32).map(String::from)
            } else if let Some(dec) = name.strip_prefix('#') {
                dec.parse::<u32>().ok().and_then(char::from_u32).map(String::from)
            } else {
                match name {
                    "amp" => Some("&"),
                    "lt" => Some("<"),
                    "gt" => Some(">"),
                    "quot" => Some("\""),
                    "apos" => Some("'"),
                    "nbsp" => Some(" "),
                    "mdash" => Some("—"),
                    "ndash" => Some("–"),
                    "rsquo" | "lsquo" => Some("'"),
                    "ldquo" | "rdquo" => Some("\""),
                    "hellip" => Some("…"),
                    _ => None,
                }
                .map(String::from)
            };
            decoded.unwrap_or_else(|| c[0].to_string())
        })
        .into_owned()
}

/// At most `limit` characters, cut at a paragraph or sentence end when one is near.
pub fn clip_chars(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let cut: String = text.chars().take(limit).collect();
    let floor = limit * 3 / 4;
    let at = cut.rfind("\n\n").filter(|&i| cut[..i].chars().count() >= floor).or_else(|| cut.rfind(". ").map(|i| i + 1).filter(|&i| cut[..i].chars().count() >= floor));
    match at {
        Some(i) => cut[..i].trim_end().to_string(),
        None => cut.trim_end().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rss_and_atom_items_are_read() {
        let rss = r#"<?xml version="1.0"?><rss xmlns:media="http://search.yahoo.com/mrss/"><channel><title>x</title>
            <item><title>Big &amp; bold&nbsp;news</title><link>https://a.example/1</link>
            <description><![CDATA[<p>Hello <b>world</b></p><img src="https://a.example/i.jpg">]]></description>
            <pubDate>Mon, 21 Sep 2026 03:23:27 GMT</pubDate></item></channel></rss>"#;
        let items = parse_feed("A", rss);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "Big & bold news");
        assert_eq!(items[0].link, "https://a.example/1");
        assert_eq!(items[0].summary, "Hello world");
        assert_eq!(items[0].image.as_deref(), Some("https://a.example/i.jpg"));
        assert!(items[0].published > 1_789_000_000);

        let atom = r#"<feed xmlns="http://www.w3.org/2005/Atom"><entry><title>T</title>
            <link rel="alternate" href="https://b.example/2"/><updated>2026-09-20T17:12:49-04:00</updated>
            <summary>S</summary></entry></feed>"#;
        let items = parse_feed("B", atom);
        assert_eq!(items[0].link, "https://b.example/2");
        assert_eq!(items[0].summary, "S");
    }

    #[test]
    fn focus_keeps_the_lead_and_wanted_sections_and_drops_references() {
        let body = "x".repeat(3000);
        let text = format!("Lead line.\n\n== Plot ==\nplot text\n\n== Production ==\n{}\n=== Filming ===\nshot in Goa\n\n== References ==\nref", body);
        let out = focus(&text, &["production"], 10_000);
        assert!(out.starts_with("Lead line."));
        assert!(out.contains("shot in Goa"));
        assert!(!out.contains("plot text"));
        assert!(!out.contains("ref\n") && !out.ends_with("ref"));
        // Too little in the wanted sections: the whole article, still without references.
        let short = "Lead.\n\n== Plot ==\nplot text\n\n== Production ==\nsmall\n\n== References ==\nref";
        let out = focus(short, &["production"], 10_000);
        assert!(out.contains("plot text") && !out.ends_with("ref"));
    }

    #[test]
    fn clipping_prefers_a_paragraph_end() {
        let text = format!("{}\n\n{}", "a".repeat(80), "b".repeat(80));
        assert_eq!(clip_chars(&text, 100), "a".repeat(80));
    }
}
