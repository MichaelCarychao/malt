use std::ops::Bound;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use tantivy::{
    collector::TopDocs,
    query::{
        AllQuery, BooleanQuery, BoostQuery, FuzzyTermQuery, Occur, Query, QueryParser, RangeQuery,
        TermQuery,
    },
    schema::{Field, IndexRecordOption, Schema, Value, FAST, INDEXED, STORED, STRING, TEXT},
    Index, IndexReader, ReloadPolicy, TantivyDocument, Term,
};

pub struct NoteIndex {
    index: Index,
    reader: IndexReader,
    /// ONE long-lived writer, created at startup. Creating an IndexWriter
    /// is heavyweight (it allocates the arena and spawns indexing threads),
    /// and the old code did it per `upsert` — i.e. on every 300ms-debounced
    /// autosave. The Mutex doubles as the single-writer serialization that
    /// `rebuild_lock` used to provide.
    writer: Mutex<tantivy::IndexWriter<TantivyDocument>>,
    path_field: Field,
    title_field: Field,
    body_field: Field,
    tags_field: Field,
    modified_field: Field,
    /// 0 when the note has meaningful content, 1 when it's empty (no body
    /// after stripping malt-private markup). Used for `empty:true` queries.
    empty_field: Field,
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
        let empty_field = sb.add_u64_field("empty", INDEXED | STORED | FAST);
        let schema = sb.build();
        let index = Index::create_in_ram(schema);
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::OnCommitWithDelay)
            .try_into()?;
        let writer = index.writer(15_000_000)?;
        Ok(Self {
            index,
            reader,
            writer: Mutex::new(writer),
            path_field,
            title_field,
            body_field,
            tags_field,
            modified_field,
            empty_field,
        })
    }

    /// Build the single tantivy document for the note `summary` with its
    /// full file `content` (from the note cache — no disk read; the old
    /// version re-read every file here, doubling each rebuild's IO).
    /// Factored out so the full-rebuild and the incremental `upsert` paths
    /// produce byte-identical documents (same schema, same tag expansion,
    /// same encrypted-note handling). `summary.path` is the document's
    /// primary key — every add MUST be preceded by `delete_term(path)`
    /// (see `upsert`/`remove`) or duplicate documents accumulate.
    fn build_doc(&self, summary: &crate::notes::NoteSummary, content: &str) -> TantivyDocument {
        let mut doc = TantivyDocument::default();
        doc.add_text(self.path_field, &summary.path);
        doc.add_text(self.title_field, &summary.title);
        if summary.is_encrypted {
            // Encrypted notes index by title only — the body is
            // ciphertext, indexing it would leak no information but
            // would also produce nonsense matches. Skip body + tag
            // fields entirely. They're still findable by filename.
            doc.add_u64(self.modified_field, summary.modified);
            doc.add_u64(self.empty_field, 0);
        } else {
            let (_fm, body) = crate::frontmatter::split(content);
            let tags = crate::tags::extract_tags_full(content);
            let expanded = crate::tags::expand_with_slash_parents(&tags);
            doc.add_text(self.body_field, body);
            for tag in &expanded {
                doc.add_text(self.tags_field, tag);
            }
            doc.add_u64(self.modified_field, summary.modified);
            doc.add_u64(self.empty_field, if summary.is_empty { 1 } else { 0 });
        }
        doc
    }

    pub fn rebuild(&self) -> tantivy::Result<()> {
        let mut writer = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        writer.delete_all_documents()?;
        for (note, content) in crate::notes::list_with_content() {
            writer.add_document(self.build_doc(&note, &content))?;
        }
        writer.commit()?;
        self.reader.reload()?;
        Ok(())
    }

    /// Incrementally (re)index a single note by path. Called synchronously
    /// from the write commands (save / create / rename) so an in-app edit is
    /// searchable immediately, without waiting for the debounced watcher
    /// rebuild — and resiliently, since dropped filesystem events can't
    /// leave the index stale for content the app itself wrote.
    ///
    /// Doubles as the cache-invalidation funnel: `refresh_path` re-reads
    /// exactly this one file into the note cache, and the fresh
    /// (summary, content) feeds the document — so every command that
    /// upserts the index keeps the sidebar/search cache current for free.
    ///
    /// `path` is the primary key: the `path` field is STRING (one exact
    /// token), so we MUST `delete_term` it before re-adding, or repeated
    /// upserts would accumulate duplicate documents for the same note.
    /// If the file is gone / unreadable / not a note, this degrades to a
    /// `remove` so a missing file never lingers in the index.
    pub fn upsert(&self, path: &str) -> tantivy::Result<()> {
        let fresh = crate::notes::refresh_path(path);
        let mut writer = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        let term = Term::from_field_text(self.path_field, path);
        writer.delete_term(term);
        if let Some((summary, content)) = fresh {
            writer.add_document(self.build_doc(&summary, &content))?;
        }
        writer.commit()?;
        self.reader.reload()?;
        Ok(())
    }

    /// Remove a single note from the index by path (its primary key). Used
    /// when a note is deleted or moved/renamed away. Also drops the path
    /// from the note cache. delete_term + commit + reload; a no-op if the
    /// path isn't present.
    pub fn remove(&self, path: &str) -> tantivy::Result<()> {
        crate::notes::forget_path_cache(path);
        let mut writer = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        let term = Term::from_field_text(self.path_field, path);
        writer.delete_term(term);
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

    /// Paths of notes whose body contains `phrase` (as a tokenized phrase
    /// match). Used to scope the unlinked-mention scan: instead of reading
    /// every note in the vault, we read only the handful the index says
    /// could possibly contain the title. This is what keeps the feature
    /// O(matches) rather than O(corpus) on large vaults.
    ///
    /// Title-cased / punctuated phrases are handled by the query parser's
    /// tokenizer (same one the body field is indexed with), so casing and
    /// surrounding punctuation don't matter.
    pub fn notes_containing(&self, phrase: &str) -> tantivy::Result<Vec<String>> {
        let trimmed = phrase.trim();
        if trimmed.is_empty() {
            return Ok(Vec::new());
        }
        let searcher = self.reader.searcher();
        let parser = QueryParser::for_index(&self.index, vec![self.body_field]);
        // Wrap in quotes for a phrase query; strip any embedded quotes so
        // the parser can't choke on unbalanced ones.
        let sanitized = trimmed.replace('"', " ");
        let query = match parser.parse_query(&format!("\"{sanitized}\"")) {
            Ok(q) => q,
            // Parser rejects pure-punctuation / empty-after-tokenize input.
            Err(_) => return Ok(Vec::new()),
        };
        let top = searcher.search(&query, &TopDocs::with_limit(1000))?;
        let mut paths = Vec::with_capacity(top.len());
        for (_score, addr) in top {
            let doc: TantivyDocument = searcher.doc(addr)?;
            if let Some(s) = doc.get_first(self.path_field).and_then(|v| v.as_str()) {
                paths.push(s.to_string());
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
            "empty" => {
                let want_empty = matches!(value.to_lowercase().as_str(), "true" | "1" | "yes");
                let v = if want_empty { 1u64 } else { 0u64 };
                Some(Box::new(TermQuery::new(
                    Term::from_field_u64(self.empty_field, v),
                    IndexRecordOption::Basic,
                )) as Box<dyn Query>)
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal NoteSummary for index tests. We index by title here (the
    /// body field is read from disk by `build_doc` for non-encrypted notes;
    /// a synthetic path that doesn't exist yields an empty body, which is
    /// fine — the title is what we assert on).
    fn summary(path: &str, title: &str) -> crate::notes::NoteSummary {
        crate::notes::NoteSummary {
            path: path.to_string(),
            title: title.to_string(),
            display_title: title.to_string(),
            snippet: String::new(),
            modified: 0,
            tags: Vec::new(),
            is_conflict: false,
            is_empty: false,
            is_encrypted: true, // title-only: skips the disk read in build_doc
            title_matches: Vec::new(),
            snippet_matches: Vec::new(),
        }
    }

    /// Index one document directly (bypassing the note cache) so these
    /// tests don't depend on the active vault. Mirrors upsert's
    /// primary-key discipline: delete_term(path) before add.
    fn index_doc(idx: &NoteIndex, s: &crate::notes::NoteSummary) {
        let mut writer = idx.writer.lock().unwrap_or_else(|e| e.into_inner());
        writer.delete_term(Term::from_field_text(idx.path_field, &s.path));
        writer.add_document(idx.build_doc(s, "")).unwrap();
        writer.commit().unwrap();
        idx.reader.reload().unwrap();
    }

    #[test]
    fn upsert_then_search_finds_remove_then_search_misses() {
        let idx = NoteIndex::new().unwrap();
        let path = "/tmp/malt-test/znglbrt.md";

        // Insert → searchable by its (unusual) title token.
        index_doc(&idx, &summary(path, "znglbrt"));
        let hits = idx.search("znglbrt", 10).unwrap();
        assert!(hits.contains(&path.to_string()), "doc should be found after insert: {hits:?}");

        // Remove → no longer searchable.
        idx.remove(path).unwrap();
        let hits = idx.search("znglbrt", 10).unwrap();
        assert!(!hits.contains(&path.to_string()), "doc should be gone after remove: {hits:?}");
    }

    #[test]
    fn reindexing_same_path_does_not_duplicate() {
        let idx = NoteIndex::new().unwrap();
        let path = "/tmp/malt-test/dup.md";

        // Index the same path twice (each insert delete_terms first, as
        // upsert does) — the path is the primary key, so there must be
        // exactly one document, not two.
        index_doc(&idx, &summary(path, "uniquetitletok"));
        index_doc(&idx, &summary(path, "uniquetitletok"));
        let hits = idx.search("uniquetitletok", 10).unwrap();
        let count = hits.iter().filter(|p| *p == path).count();
        assert_eq!(count, 1, "path is the primary key — no duplicate docs: {hits:?}");

        // And remove clears it entirely.
        idx.remove(path).unwrap();
        let hits = idx.search("uniquetitletok", 10).unwrap();
        assert!(hits.is_empty(), "should be empty after remove: {hits:?}");
    }
}
