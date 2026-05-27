use std::ops::Bound;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use tantivy::{
    collector::TopDocs,
    query::{AllQuery, BooleanQuery, BoostQuery, FuzzyTermQuery, Occur, Query, RangeQuery, TermQuery},
    schema::{Field, IndexRecordOption, Schema, Value, FAST, INDEXED, STORED, STRING, TEXT},
    Index, IndexReader, ReloadPolicy, TantivyDocument, Term,
};

pub struct NoteIndex {
    index: Index,
    reader: IndexReader,
    path_field: Field,
    title_field: Field,
    body_field: Field,
    tags_field: Field,
    modified_field: Field,
    rebuild_lock: Mutex<()>,
}

impl NoteIndex {
    pub fn new() -> tantivy::Result<Self> {
        let mut sb = Schema::builder();
        let path_field = sb.add_text_field("path", STRING | STORED);
        let title_field = sb.add_text_field("title", TEXT);
        let body_field = sb.add_text_field("body", TEXT);
        // STRING (not TEXT) tokenizer: each tag is a single exact token so
        // `tag:foo` is an exact match, never fuzzy. Multi-value via repeated
        // add_text() calls per doc.
        let tags_field = sb.add_text_field("tags", STRING);
        let modified_field = sb.add_u64_field("modified", INDEXED | STORED | FAST);
        let schema = sb.build();
        let index = Index::create_in_ram(schema);
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::OnCommitWithDelay)
            .try_into()?;
        Ok(Self {
            index,
            reader,
            path_field,
            title_field,
            body_field,
            tags_field,
            modified_field,
            rebuild_lock: Mutex::new(()),
        })
    }

    pub fn rebuild(&self) -> tantivy::Result<()> {
        let _g = self.rebuild_lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut writer = self.index.writer(15_000_000)?;
        writer.delete_all_documents()?;
        for note in crate::notes::list_notes() {
            let content = std::fs::read_to_string(&note.path).unwrap_or_default();
            let (_fm, body) = crate::frontmatter::split(&content);
            let tags = crate::tags::extract_tags_full(&content);
            let expanded = crate::tags::expand_with_slash_parents(&tags);
            let mut doc = TantivyDocument::default();
            doc.add_text(self.path_field, &note.path);
            doc.add_text(self.title_field, &note.title);
            doc.add_text(self.body_field, body);
            for tag in &expanded {
                doc.add_text(self.tags_field, tag);
            }
            doc.add_u64(self.modified_field, note.modified);
            writer.add_document(doc)?;
        }
        writer.commit()?;
        self.reader.reload()?;
        Ok(())
    }

    pub fn search(&self, raw_query: &str, limit: usize) -> tantivy::Result<Vec<String>> {
        let searcher = self.reader.searcher();
        let q = self.build_query(raw_query);
        let top = searcher.search(&q, &TopDocs::with_limit(limit))?;
        let mut paths = Vec::with_capacity(top.len());
        for (_score, addr) in top {
            let doc: TantivyDocument = searcher.doc(addr)?;
            if let Some(v) = doc.get_first(self.path_field) {
                if let Some(s) = v.as_str() {
                    paths.push(s.to_string());
                }
            }
        }
        Ok(paths)
    }

    fn build_query(&self, raw: &str) -> Box<dyn Query> {
        let tokens: Vec<String> = raw
            .split_whitespace()
            .map(|t| t.to_string())
            .filter(|t| !t.is_empty())
            .collect();
        if tokens.is_empty() {
            return Box::new(AllQuery);
        }

        let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::with_capacity(tokens.len());
        for tok in tokens {
            let (occur, body) = parse_negation(&tok);
            // Operator form: field:value
            if let Some((field, value)) = body.split_once(':') {
                if let Some(q) = self.build_operator_query(field, value) {
                    clauses.push((occur, q));
                    continue;
                }
                // Unknown operator — fall through to treating the whole token
                // as a fuzzy text term so we don't silently ignore it.
            }
            clauses.push((occur, self.build_text_query(body)));
        }
        // If every clause was a MustNot, Tantivy needs an AllQuery to MUST against.
        if clauses.iter().all(|(o, _)| matches!(o, Occur::MustNot)) {
            clauses.push((Occur::Must, Box::new(AllQuery)));
        }
        Box::new(BooleanQuery::new(clauses))
    }

    fn build_operator_query(&self, field: &str, value: &str) -> Option<Box<dyn Query>> {
        match field.to_lowercase().as_str() {
            "tag" => {
                let canon = crate::tags::canonicalize(value);
                if canon.is_empty() {
                    return None;
                }
                let term = Term::from_field_text(self.tags_field, &canon);
                Some(Box::new(TermQuery::new(term, IndexRecordOption::Basic)))
            }
            "modified" => parse_relative_time(value).map(|range| {
                Box::new(RangeQuery::new(
                    Bound::Included(Term::from_field_u64(self.modified_field, range.0)),
                    Bound::Included(Term::from_field_u64(self.modified_field, range.1)),
                )) as Box<dyn Query>
            }),
            _ => None,
        }
    }

    fn build_text_query(&self, term: &str) -> Box<dyn Query> {
        let lower = term.to_lowercase();
        let dist = fuzzy_distance(lower.len());
        let title_term = Term::from_field_text(self.title_field, &lower);
        let body_term = Term::from_field_text(self.body_field, &lower);
        let title_q: Box<dyn Query> = Box::new(BoostQuery::new(
            Box::new(FuzzyTermQuery::new_prefix(title_term, dist, true)),
            3.0,
        ));
        let body_q: Box<dyn Query> = Box::new(FuzzyTermQuery::new_prefix(body_term, dist, true));
        Box::new(BooleanQuery::new(vec![
            (Occur::Should, title_q),
            (Occur::Should, body_q),
        ]))
    }
}

fn parse_negation(tok: &str) -> (Occur, &str) {
    if let Some(rest) = tok.strip_prefix('-') {
        if !rest.is_empty() {
            return (Occur::MustNot, rest);
        }
    }
    (Occur::Must, tok)
}

/// Parse `modified:` values. Accepts:
///   `<7d`, `<24h`, `<1w` — within the last N units
///   `>30d`, `>1y`        — older than N units
///   `7d`                  — alias for `<7d`
/// Returns (min_ts, max_ts) inclusive bounds.
fn parse_relative_time(raw: &str) -> Option<(u64, u64)> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .ok()?;
    let (older, rest) = if let Some(r) = raw.strip_prefix('<') {
        (false, r)
    } else if let Some(r) = raw.strip_prefix('>') {
        (true, r)
    } else {
        (false, raw)
    };
    let (n_str, unit) = rest.split_at(rest.len().saturating_sub(1));
    let n: u64 = n_str.parse().ok()?;
    let secs = match unit {
        "h" => n * 3600,
        "d" => n * 86400,
        "w" => n * 86400 * 7,
        "m" => n * 86400 * 30,
        "y" => n * 86400 * 365,
        _ => return None,
    };
    if older {
        // older than N units ago → modified ∈ [0, now - secs]
        Some((0, now.saturating_sub(secs)))
    } else {
        // within last N units → modified ∈ [now - secs, ∞)
        Some((now.saturating_sub(secs), u64::MAX))
    }
}

fn fuzzy_distance(len: usize) -> u8 {
    match len {
        0..=3 => 0,
        4..=6 => 1,
        _ => 2,
    }
}
