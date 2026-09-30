//! Citation formatting (Phase 5, spec §31).
//!
//! Deterministic formatting from structured bibliographic metadata — an LLM
//! never invents a formatted reference (spec §31). Styles implemented here:
//! APA 7, Harvard and Chicago (author-date). The [`CitationFormatter`]
//! interface keeps the style engine swappable (e.g. a future CSL engine) —
//! see docs/CITATIONS.md for the Citation.js/AGPL licensing decision.
//!
//! Author parsing: `documents.authors` is a display string, typically
//! "Jane Doe; John Smith" (semicolon or " and " separated). Formats degrade
//! gracefully: missing fields are simply omitted, never guessed.

use serde::Serialize;

use crate::services::documents::DocumentRow;

/// Reference types the formatter understands (spec §31).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RefType {
    Article,
    Book,
    Chapter,
    Report,
    Webpage,
    Thesis,
}

impl RefType {
    pub fn from_str(s: &str) -> Self {
        match s {
            "book" => RefType::Book,
            "chapter" => RefType::Chapter,
            "report" => RefType::Report,
            "webpage" => RefType::Webpage,
            "thesis" => RefType::Thesis,
            _ => RefType::Article,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            RefType::Article => "article",
            RefType::Book => "book",
            RefType::Chapter => "chapter",
            RefType::Report => "report",
            RefType::Webpage => "webpage",
            RefType::Thesis => "thesis",
        }
    }
}

/// Selectable styles (spec §31: APA 7, Harvard, Chicago initial; CSL later).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CitationStyle {
    Apa,
    Harvard,
    Chicago,
}

impl CitationStyle {
    pub fn from_str(s: &str) -> AppResult<Self> {
        Ok(match s {
            "apa" => CitationStyle::Apa,
            "harvard" => CitationStyle::Harvard,
            "chicago" => CitationStyle::Chicago,
            other => {
                return Err(crate::error::AppError::msg(format!(
                    "Unknown citation style \"{other}\". Expected apa, harvard or chicago."
                )))
            }
        })
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            CitationStyle::Apa => "apa",
            CitationStyle::Harvard => "harvard",
            CitationStyle::Chicago => "chicago",
        }
    }
}

use crate::error::AppResult;

/// The swappable formatting seam (spec §47.7).
pub trait CitationFormatter: Send + Sync {
    /// Full reference-list entry.
    fn reference(&self, doc: &DocumentRow, style: CitationStyle) -> String;
    /// Short in-text citation, e.g. "(Smith, 2021)".
    fn in_text(&self, doc: &DocumentRow, style: CitationStyle) -> String;
}

/// Deterministic built-in formatter (no external deps, MIT-clean).
pub struct BuiltinFormatter;

impl CitationFormatter for BuiltinFormatter {
    fn reference(&self, doc: &DocumentRow, style: CitationStyle) -> String {
        let authors = parse_authors(doc.authors.as_deref().unwrap_or(""));
        match style {
            CitationStyle::Apa => apa_reference(doc, &authors),
            CitationStyle::Harvard => harvard_reference(doc, &authors),
            CitationStyle::Chicago => chicago_reference(doc, &authors),
        }
    }

    fn in_text(&self, doc: &DocumentRow, style: CitationStyle) -> String {
        let authors = parse_authors(doc.authors.as_deref().unwrap_or(""));
        let surname = authors
            .first()
            .map(|a| a.family.clone())
            .unwrap_or_else(|| doc.title.as_deref().unwrap_or(&doc.file_name).to_string());
        let et_al = if authors.len() > 2 { " et al." } else { "" };
        let year = doc
            .year
            .map(|y| y.to_string())
            .unwrap_or_else(|| "n.d.".into());
        // Two named authors join inside the parens; three+ collapse to et al.
        let name = match (style, authors.len()) {
            (CitationStyle::Apa, 2) => {
                format!("{} & {}", authors[0].family, authors[1].family)
            }
            (CitationStyle::Harvard, 2) => {
                format!("{} and {}", authors[0].family, authors[1].family)
            }
            _ => format!("{surname}{et_al}"),
        };
        format!("({name}, {year})")
    }
}

// ---------------------------------------------------------------------------
// Author model
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Author {
    family: String,
    given: String,
}

/// Parse "Jane Doe; John Smith" / "Jane Doe and John Smith" /
/// "Doe, J.; Smith, J." into structured authors. Best-effort: an entry
/// without a recognisable split becomes family-only.
fn parse_authors(raw: &str) -> Vec<Author> {
    raw.split([';', '&'])
        .flat_map(|part| part.split(" and "))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|entry| {
            if let Some((family, given)) = entry.split_once(',') {
                Author {
                    family: family.trim().to_string(),
                    given: given.trim().to_string(),
                }
            } else {
                let mut parts = entry.split_whitespace();
                let family = parts.next_back().unwrap_or(entry).to_string();
                let given = parts.collect::<Vec<_>>().join(" ");
                Author { family, given }
            }
        })
        .collect()
}

fn initials(given: &str) -> String {
    given
        .split([' ', '.', '-'])
        .filter(|p| !p.is_empty())
        .map(|p| format!("{}.", p.chars().next().unwrap_or(' ').to_ascii_uppercase()))
        .collect::<Vec<_>>()
        .join(" ")
}

// ---------------------------------------------------------------------------
// Style implementations
// ---------------------------------------------------------------------------

fn container(doc: &DocumentRow) -> Option<&str> {
    doc.journal.as_deref().or(doc.publisher.as_deref())
}

fn apa_reference(doc: &DocumentRow, authors: &[Author]) -> String {
    let mut out = String::new();
    match authors.len() {
        0 => {}
        1 => out.push_str(&apa_name(&authors[0])),
        2 => out.push_str(&format!(
            "{}, & {}",
            apa_name(&authors[0]),
            apa_name(&authors[1])
        )),
        _ => out.push_str(&format!(
            "{}, et al.",
            apa_name(&authors[0])
        )),
    }
    // Author block always ends with a full stop (initials already carry one).
    if !out.is_empty() && !out.ends_with('.') {
        out.push('.');
    }
    if !out.is_empty() {
        out.push(' ');
    }
    let year = doc.year.map(|y| y.to_string()).unwrap_or_else(|| "n.d.".into());
    out.push_str(&format!("({year}). "));

    let title = doc
        .title
        .clone()
        .unwrap_or_else(|| doc.file_name.clone());
    match RefType::from_str(&doc.ref_type) {
        RefType::Book => {
            out.push_str(&format!("_{}_.", title));
            if let Some(publisher) = doc.publisher.as_deref() {
                out.push_str(&format!(" {publisher}."));
            }
        }
        RefType::Webpage => {
            out.push_str(&format!("{}. ", title));
            if let Some(url) = doc.url.as_deref() {
                out.push_str(&url.to_string());
            }
        }
        _ => {
            out.push_str(&format!("{}. ", title));
            if let Some(journal) = doc.journal.as_deref() {
                out.push_str(&format!("_{}_", journal));
                if let Some(vol) = doc.volume.as_deref() {
                    out.push_str(&format!(", _{vol}_"));
                    if let Some(issue) = doc.issue.as_deref() {
                        out.push_str(&format!("({issue})"));
                    }
                }
                if let Some(pages) = doc.pages.as_deref() {
                    out.push_str(&format!(", {pages}"));
                }
                out.push('.');
            } else if let Some(publisher) = doc.publisher.as_deref() {
                out.push_str(&format!(" {publisher}."));
            }
        }
    }
    if let Some(doi) = doc.doi.as_deref() {
        out.push_str(&format!(" https://doi.org/{doi}"));
    }
    out
}

fn apa_name(a: &Author) -> String {
    if a.given.is_empty() {
        a.family.clone()
    } else {
        format!("{}, {}", a.family, initials(&a.given))
    }
}

fn harvard_reference(doc: &DocumentRow, authors: &[Author]) -> String {
    let mut out = String::new();
    if !authors.is_empty() {
        let names: Vec<String> = authors
            .iter()
            .map(|a| {
                if a.given.is_empty() {
                    a.family.clone()
                } else {
                    format!("{}, {}", a.family, initials(&a.given))
                }
            })
            .collect();
        out.push_str(&names.join(" and "));
        out.push(' ');
    }
    let year = doc.year.map(|y| y.to_string()).unwrap_or_else(|| "n.d.".into());
    out.push_str(&format!("({year}) "));

    let title = doc.title.clone().unwrap_or_else(|| doc.file_name.clone());
    match RefType::from_str(&doc.ref_type) {
        RefType::Book => {
            out.push_str(&format!("_{title}_."));
            if let Some(publisher) = doc.publisher.as_deref() {
                let _ = publisher;
            }
        }
        _ => {
            out.push_str(&format!("‘{}’, ", title));
            if let Some(j) = container(doc) {
                out.push_str(&format!("_{j}_"));
                if let Some(vol) = doc.volume.as_deref() {
                    out.push_str(&format!(", {vol}"));
                    if let Some(issue) = doc.issue.as_deref() {
                        out.push_str(&format!("({issue})"));
                    }
                }
                if let Some(pages) = doc.pages.as_deref() {
                    out.push_str(&format!(", pp. {pages}"));
                }
                out.push('.');
            }
        }
    }
    if let Some(doi) = doc.doi.as_deref() {
        out.push_str(&format!(" doi: {doi}."));
    }
    out
}

fn chicago_reference(doc: &DocumentRow, authors: &[Author]) -> String {
    let mut out = String::new();
    if !authors.is_empty() {
        let names: Vec<String> = authors
            .iter()
            .take(1)
            .map(|a| {
                if a.given.is_empty() {
                    a.family.clone()
                } else {
                    format!("{}, {}", a.family, a.given)
                }
            })
            .collect();
        out.push_str(&names.join(", "));
        if authors.len() > 1 {
            out.push_str(", et al.");
        }
        if !out.ends_with('.') {
            out.push('.');
        }
        out.push(' ');
    }

    let title = doc.title.clone().unwrap_or_else(|| doc.file_name.clone());
    match RefType::from_str(&doc.ref_type) {
        RefType::Book => {
            out.push_str(&format!("_{title}_."));
            if let Some(publisher) = doc.publisher.as_deref() {
                let year = doc.year.map(|y| y.to_string()).unwrap_or_default();
                out.push_str(&format!(" {publisher}, {year}."));
            }
        }
        _ => {
            out.push_str(&format!("“{title}.” "));
            if let Some(j) = container(doc) {
                out.push_str(&format!("_{j}_"));
                if let Some(vol) = doc.volume.as_deref() {
                    out.push_str(&format!(" {vol}"));
                    if let Some(issue) = doc.issue.as_deref() {
                        out.push_str(&format!(", no. {issue}"));
                    }
                }
                if let Some(year) = doc.year {
                    out.push_str(&format!(" ({year})"));
                }
                if let Some(pages) = doc.pages.as_deref() {
                    out.push_str(&format!(": {pages}"));
                }
                out.push('.');
            }
        }
    }
    if let Some(doi) = doc.doi.as_deref() {
        out.push_str(&format!(" https://doi.org/{doi}."));
    }
    out
}

/// A formatted reference ready for the UI/export layer.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FormattedReference {
    pub document_id: String,
    pub style: String,
    pub reference: String,
    pub in_text: String,
    /// True when key fields (title/year) are missing — the UI can nudge the
    /// user to correct metadata (spec §31: user corrections are authoritative).
    pub incomplete: bool,
}

/// Format one document in one style.
pub fn format_reference(doc: &DocumentRow, style: CitationStyle) -> FormattedReference {
    let formatter = BuiltinFormatter;
    let incomplete = doc.title.is_none() || doc.year.is_none();
    FormattedReference {
        document_id: doc.id.clone(),
        style: style.as_str().to_string(),
        reference: formatter.reference(doc, style),
        in_text: formatter.in_text(doc, style),
        incomplete,
    }
}

/// Formatted reference list for a whole project, alphabetised by first
/// author surname (or title when no authors), as bibliographies require.
pub fn bibliography(db: &crate::db::Db, project_id: &str, style: CitationStyle) -> AppResult<Vec<FormattedReference>> {
    // Collect ids under a scoped lock — the per-document fetch below re-locks
    // via get_document, so the guard MUST be dropped first (mutex is not
    // re-entrant; holding it here would self-deadlock).
    let ids = {
        let conn = db.lock();
        let mut stmt = conn.prepare(
            "SELECT id FROM documents WHERE project_id = ?1 ORDER BY file_name ASC",
        )?;
        let ids = stmt
            .query_map([project_id], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        ids
    };

    let mut refs: Vec<(String, FormattedReference)> = Vec::new();
    for id in ids {
        let doc = db.get_document(&id)?;
        refs.push((id, format_reference(&doc, style)));
    }
    // Alphabetical by the formatted reference (authors-first shape); the id
    // tiebreak keeps the sort stable across equal keys.
    refs.sort_by(|a, b| a.0.cmp(&b.0));
    refs.sort_by(|a, b| a.1.reference.to_lowercase().cmp(&b.1.reference.to_lowercase()));
    Ok(refs.into_iter().map(|(_, r)| r).collect())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::tests::temp_dir_for;

    fn doc(fields: impl FnOnce(&mut DocumentRow)) -> DocumentRow {
        let mut d = DocumentRow {
            id: "d1".into(),
            project_id: "p1".into(),
            file_name: "paper.pdf".into(),
            original_path: "/tmp/paper.pdf".into(),
            managed_path: None,
            document_type: "pdf".into(),
            checksum: "c".into(),
            title: Some("Delta retreat under sea-level rise".into()),
            authors: Some("Jane Doe; John A. Smith".into()),
            year: Some(2021),
            doi: Some("10.1234/deltas".into()),
            journal: Some("Coastal Research".into()),
            volume: Some("12".into()),
            issue: Some("3".into()),
            pages: Some("101–118".into()),
            publisher: None,
            url: None,
            ref_type: "article".into(),
            indexing_status: "ready".into(),
            status_detail: None,
            page_count: Some(18),
            language: None,
            imported_at: "t".into(),
            chunk_count: 0,
        };
        fields(&mut d);
        d
    }

    #[test]
    fn apa7_reference_matches_expected_shape() {
        let d = doc(|_| {});
        let r = format_reference(&d, CitationStyle::Apa);
        assert_eq!(
            r.reference,
            "Doe, J., & Smith, J. A. (2021). Delta retreat under sea-level rise. _Coastal Research_, _12_(3), 101–118. https://doi.org/10.1234/deltas"
        );
        assert_eq!(r.in_text, "(Doe & Smith, 2021)");
        assert!(!r.incomplete);
    }

    #[test]
    fn harvard_reference_matches_expected_shape() {
        let d = doc(|_| {});
        let r = format_reference(&d, CitationStyle::Harvard);
        assert_eq!(
            r.reference,
            "Doe, J. and Smith, J. A. (2021) ‘Delta retreat under sea-level rise’, _Coastal Research_, 12(3), pp. 101–118. doi: 10.1234/deltas."
        );
        assert_eq!(r.in_text, "(Doe and Smith, 2021)");
    }

    #[test]
    fn chicago_reference_matches_expected_shape() {
        let d = doc(|_| {});
        let r = format_reference(&d, CitationStyle::Chicago);
        assert_eq!(
            r.reference,
            "Doe, Jane, et al. “Delta retreat under sea-level rise.” _Coastal Research_ 12, no. 3 (2021): 101–118. https://doi.org/10.1234/deltas."
        );
    }

    #[test]
    fn book_and_webpage_variants() {
        let book = doc(|d| {
            d.title = Some("Coastal Dynamics".into());
            d.authors = Some("A. Author".into());
            d.publisher = Some("Academic Press".into());
            d.journal = None;
            d.doi = None;
            d.ref_type = "book".into();
        });
        let r = format_reference(&book, CitationStyle::Apa);
        assert!(r.reference.contains("_Coastal Dynamics_. Academic Press."), "{}", r.reference);

        let web = doc(|d| {
            d.title = Some("Adaptation report".into());
            d.url = Some("https://example.org/report".into());
            d.journal = None;
            d.doi = None;
            d.ref_type = "webpage".into();
        });
        let r = format_reference(&web, CitationStyle::Apa);
        assert!(r.reference.contains("https://example.org/report"), "{}", r.reference);
    }

    #[test]
    fn missing_metadata_degrades_and_flags_incomplete() {
        let d = doc(|d| {
            d.authors = None;
            d.year = None;
        });
        let r = format_reference(&d, CitationStyle::Apa);
        assert!(r.reference.starts_with("(n.d.)."));
        assert!(r.incomplete);
        let in_text = r.in_text;
        assert!(in_text.contains("Delta") || in_text.contains("n.d."), "{in_text}");
    }

    #[test]
    fn author_parsing_handles_common_shapes() {
        let authors = parse_authors("Doe, Jane; Smith, J. and Lee, A & Wong, B");
        assert_eq!(authors.len(), 4);
        assert_eq!(authors[0].family, "Doe");
        assert_eq!(authors[0].given, "Jane");
        assert_eq!(authors[1].family, "Smith");
        assert_eq!(parse_authors("").len(), 0);
        let single = parse_authors("Cher");
        assert_eq!(single[0].family, "Cher");
    }

    #[test]
    fn style_parsing_and_three_author_in_text() {
        assert!(CitationStyle::from_str("mla").is_err());
        assert_eq!(CitationStyle::from_str("harvard").unwrap(), CitationStyle::Harvard);

        let d = doc(|d| {
            d.authors = Some("A. One; B. Two; C. Three".into());
        });
        let r = format_reference(&d, CitationStyle::Apa);
        assert_eq!(r.in_text, "(One et al., 2021)");
    }

    #[test]
    fn bibliography_is_sorted_and_complete() {
        let dir = temp_dir_for("citations-bib");
        let db = crate::db::Db::open(dir.path()).unwrap();
        let project = db.create_project("Bib", None).unwrap();

        for (name, author) in [
            ("zeta.pdf", "Z. Zeta"),
            ("alpha.pdf", "A. Alpha"),
            ("mid.pdf", "M. Mid"),
        ] {
            let id = uuid::Uuid::new_v4().to_string();
            db.insert_document(DocumentRow {
                id: id.clone(),
                project_id: project.id.clone(),
                file_name: name.into(),
                original_path: format!("/tmp/{name}"),
                managed_path: None,
                document_type: "pdf".into(),
                checksum: uuid::Uuid::new_v4().to_string(),
                title: Some(format!("Title {name}")),
                authors: Some(author.into()),
                year: Some(2020),
                doi: None,
                journal: None,
                volume: None,
                issue: None,
                pages: None,
                publisher: Some("Press".into()),
                url: None,
                ref_type: "article".into(),
                indexing_status: "ready".into(),
                status_detail: None,
                page_count: None,
                language: None,
                imported_at: crate::db::now_iso_pub(),
                chunk_count: 0,
            })
            .unwrap();
        }

        let bib = bibliography(&db, &project.id, CitationStyle::Apa).unwrap();
        assert_eq!(bib.len(), 3);
        assert!(bib[0].reference.contains("Alpha"), "{}", bib[0].reference);
        assert!(bib[2].reference.contains("Zeta"), "{}", bib[2].reference);
    }
}
