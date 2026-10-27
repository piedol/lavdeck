//! The Recordings list: filters, search, sort, the rows' text, and what multi-selection
//! actions work on. Pure, so it's tested without a window.

use std::cmp::Ordering;
use std::collections::HashMap;

use super::takes::{Take, TranscriptDocument};
use crate::format;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filter {
    All,
    Video,
    Transcribed,
    Untranscribed,
    TwoSpeaker,
}

impl Filter {
    pub const ALL: [Filter; 5] = [
        Filter::All,
        Filter::Video,
        Filter::Transcribed,
        Filter::Untranscribed,
        Filter::TwoSpeaker,
    ];

    pub fn keeps(self, take: &Take) -> bool {
        match self {
            Filter::All => true,
            Filter::Video => take.media_rank() > 0,
            Filter::Transcribed => take.has_transcript(),
            Filter::Untranscribed => !take.has_transcript(),
            Filter::TwoSpeaker => take.is_two_speaker(),
        }
    }
}

/// A sortable column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Column {
    Recorded,
    Length,
    Video,
    Mics,
    Transcript,
}

impl Column {
    pub const ALL: [Column; 5] = [
        Column::Recorded,
        Column::Length,
        Column::Video,
        Column::Mics,
        Column::Transcript,
    ];

    /// The table column identifier and sort key.
    pub fn key(self) -> &'static str {
        match self {
            Column::Recorded => "started",
            Column::Length => "duration",
            Column::Video => "media",
            Column::Mics => "speakers",
            Column::Transcript => "snippet",
        }
    }

    pub fn from_key(key: &str) -> Option<Column> {
        Column::ALL.into_iter().find(|c| c.key() == key)
    }
}

/// One sort criterion; the table keeps a list, first one wins, the rest break ties.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SortKey {
    pub column: Column,
    pub ascending: bool,
}

/// Newest first, as the window opens.
pub const DEFAULT_SORT: SortKey = SortKey {
    column: Column::Recorded,
    ascending: false,
};

/// A take as a table row.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub take: Take,
    pub snippet: String,
}

impl Row {
    pub fn id(&self) -> &str {
        &self.take.id
    }

    pub fn started(&self) -> f64 {
        self.take.started.unwrap_or(f64::MIN)
    }

    pub fn duration(&self) -> f64 {
        self.take.duration.unwrap_or(0.0)
    }

    pub fn speakers(&self) -> String {
        self.take.speaker_names()
    }

    fn compare(&self, other: &Row, column: Column) -> Ordering {
        match column {
            Column::Recorded => self.started().total_cmp(&other.started()),
            Column::Length => self.duration().total_cmp(&other.duration()),
            Column::Video => self.take.media_rank().cmp(&other.take.media_rank()),
            Column::Mics => text_order(&self.speakers(), &other.speakers()),
            Column::Transcript => text_order(&self.snippet, &other.snippet),
        }
    }
}

/// Text order for the table: case-insensitive, then exact, like Finder's.
fn text_order(a: &str, b: &str) -> Ordering {
    a.to_lowercase()
        .cmp(&b.to_lowercase())
        .then_with(|| a.cmp(b))
}

/// The first three lines of a transcript, for the list.
pub fn snippet(doc: Option<&TranscriptDocument>) -> String {
    let Some(doc) = doc else { return String::new() };
    doc.segments
        .iter()
        .take(3)
        .map(|s| s.text.as_str())
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string()
}

/// A transcript as plain text: `[00:00:07] Guest: Thanks for having me.`, with names only when
/// there's more than one speaker.
pub fn transcript_text(doc: &TranscriptDocument) -> String {
    let multi = doc.speakers.len() > 1;
    doc.segments
        .iter()
        .map(|s| {
            let who = if multi {
                format!("{}: ", doc.speaker_name(&s.speaker))
            } else {
                String::new()
            };
            format!("[{}] {who}{}", format::timestamp(s.start), s.text)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Takes' transcripts for the clipboard, oldest first, each under its date when there are
/// several. `date` writes a take's date (injected so tests don't depend on the locale).
pub fn copied_transcripts(
    takes: &[&Take],
    docs: &HashMap<String, TranscriptDocument>,
    date: impl Fn(f64) -> String,
) -> Option<String> {
    let mut ordered: Vec<&&Take> = takes.iter().collect();
    ordered.sort_by(|a, b| {
        a.started
            .unwrap_or(f64::MIN)
            .total_cmp(&b.started.unwrap_or(f64::MIN))
    });
    let several = ordered.len() > 1;
    let parts: Vec<String> = ordered
        .iter()
        .filter_map(|t| {
            let text = transcript_text(docs.get(&t.id)?);
            Some(if several {
                format!("{}\n\n{text}", date(t.started.unwrap_or(f64::MIN)))
            } else {
                text
            })
        })
        .collect();
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

/// The rows a filter and search show, sorted. Search matches mic names, the take id, the date
/// as written in the heading, and the transcript, ignoring case.
pub fn rows(
    takes: &[Take],
    docs: &HashMap<String, TranscriptDocument>,
    filter: Filter,
    search: &str,
    sort: &[SortKey],
    date: impl Fn(f64) -> String,
) -> Vec<Row> {
    let query = search.trim().to_lowercase();
    let mut out: Vec<Row> = takes
        .iter()
        .filter(|t| filter.keeps(t))
        .map(|t| Row {
            take: t.clone(),
            snippet: snippet(docs.get(&t.id)),
        })
        .filter(|row| {
            if query.is_empty() {
                return true;
            }
            row.speakers().to_lowercase().contains(&query)
                || row.take.id.contains(&query)
                || date(row.started()).to_lowercase().contains(&query)
                || docs
                    .get(&row.take.id)
                    .is_some_and(|d| d.plain_text().to_lowercase().contains(&query))
        })
        .collect();
    let sort = if sort.is_empty() {
        &[DEFAULT_SORT][..]
    } else {
        sort
    };
    out.sort_by(|a, b| {
        for key in sort {
            let o = a.compare(b, key.column);
            let o = if key.ascending { o } else { o.reverse() };
            if o != Ordering::Equal {
                return o;
            }
        }
        Ordering::Equal
    });
    out
}

/// After removing `doomed` from `visible` (in table order): the row that takes the first removed
/// row's place, else the last one left, so the list keeps its spot.
pub fn next_after_delete(visible: &[String], doomed: &[String]) -> Option<String> {
    let first = visible.iter().position(|id| doomed.contains(id))?;
    visible[first..]
        .iter()
        .find(|id| !doomed.contains(id))
        .or_else(|| visible.iter().rev().find(|id| !doomed.contains(id)))
        .cloned()
}

/// What a multi-selection's header says, in parts.
#[derive(Clone, Debug, PartialEq)]
pub struct SelectionSummary {
    pub count: usize,
    /// First and last start times, when known.
    pub span: Option<(f64, f64)>,
    pub total: f64,
    pub transcribed: usize,
    pub with_video: usize,
    pub files: usize,
}

pub fn summarize(takes: &[&Take]) -> SelectionSummary {
    let mut dates: Vec<f64> = takes.iter().filter_map(|t| t.started).collect();
    dates.sort_by(f64::total_cmp);
    SelectionSummary {
        count: takes.len(),
        span: dates.first().zip(dates.last()).map(|(a, b)| (*a, *b)),
        total: takes.iter().map(|t| t.duration.unwrap_or(0.0)).sum(),
        transcribed: takes.iter().filter(|t| t.has_transcript()).count(),
        with_video: takes.iter().filter(|t| t.media_rank() > 0).count(),
        files: takes.iter().map(|t| t.files.len()).sum(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::takes::{Role, Segment, Speaker, TakeFile, TakeSpeaker};
    use std::path::PathBuf;

    fn take(id: &str, started: f64, duration: f64, names: &[&str], extra: &[TakeFile]) -> Take {
        let folder = PathBuf::from("/r/2026-10-26");
        let mut files = vec![TakeFile::new(
            folder.join(format!("{id} Stereo.wav")),
            Role::Stereo,
        )];
        files.extend(extra.iter().cloned());
        Take {
            id: format!("2026-10-26/{id}"),
            folder,
            stem: id.into(),
            started: Some(started),
            duration: Some(duration),
            channel_mode: Some("stereo".into()),
            files,
            speakers: names
                .iter()
                .enumerate()
                .map(|(i, n)| TakeSpeaker {
                    id: format!("tx{}", i + 1),
                    mic: i as u8 + 1,
                    name: (*n).into(),
                    file: None,
                })
                .collect(),
        }
    }

    fn doc(lines: &[(&str, &str)]) -> TranscriptDocument {
        TranscriptDocument {
            version: 1,
            take: String::new(),
            model: "whisper".into(),
            language: Some("en".into()),
            speakers: vec![
                Speaker {
                    id: "tx1".into(),
                    name: "Host".into(),
                },
                Speaker {
                    id: "tx2".into(),
                    name: "Guest".into(),
                },
            ],
            segments: lines
                .iter()
                .enumerate()
                .map(|(i, (who, text))| Segment {
                    start: i as f64 * 7.0,
                    end: i as f64 * 7.0 + 6.0,
                    speaker: (*who).into(),
                    text: (*text).into(),
                })
                .collect(),
        }
    }

    fn transcript_file(id: &str) -> TakeFile {
        let mut f = TakeFile::new(format!("/r/2026-10-26/{id}.transcript.json"), Role::Derived);
        f.kind = Some("transcript".into());
        f
    }

    fn video_file(id: &str, source: &str) -> TakeFile {
        let mut f = TakeFile::new(format!("/r/2026-10-26/{id} {source}.mov"), Role::Capture);
        f.kind = Some("video".into());
        f.source = Some(source.into());
        f
    }

    fn date(t: f64) -> String {
        format!("day {t}")
    }

    fn library() -> (Vec<Take>, HashMap<String, TranscriptDocument>) {
        let a = take(
            "11.05.09",
            300.0,
            842.0,
            &["Host", "Guest"],
            &[transcript_file("11.05.09")],
        );
        let b = take(
            "18.22.40",
            200.0,
            95.0,
            &["Host"],
            &[video_file("18.22.40", "screen")],
        );
        let c = take("09.15.00", 100.0, 2650.0, &["TX1", "TX2"], &[]);
        let mut docs = HashMap::new();
        docs.insert(
            a.id.clone(),
            doc(&[
                ("tx1", "Welcome back."),
                ("tx2", "Thanks for having me."),
                ("tx1", "So, lavs."),
                ("tx2", "Hidden line"),
            ]),
        );
        (vec![a, b, c], docs)
    }

    fn ids(rows: &[Row]) -> Vec<&str> {
        rows.iter().map(|r| r.take.stem.as_str()).collect()
    }

    #[test]
    fn sorts_newest_first_by_default() {
        let (takes, docs) = library();
        let r = rows(&takes, &docs, Filter::All, "", &[], date);
        assert_eq!(ids(&r), ["11.05.09", "18.22.40", "09.15.00"]);
    }

    #[test]
    fn sorts_by_any_column_with_tiebreaks() {
        let (takes, docs) = library();
        let by = |column, ascending| {
            rows(
                &takes,
                &docs,
                Filter::All,
                "",
                &[SortKey { column, ascending }],
                date,
            )
        };
        assert_eq!(
            ids(&by(Column::Length, true)),
            ["18.22.40", "11.05.09", "09.15.00"]
        );
        assert_eq!(ids(&by(Column::Video, false))[0], "18.22.40");
        // "Host" sorts before "Host, Guest".
        assert_eq!(
            ids(&by(Column::Mics, true)),
            ["18.22.40", "11.05.09", "09.15.00"]
        );
        // Empty snippets sort first ascending; the second key breaks the tie.
        let r = rows(
            &takes,
            &docs,
            Filter::All,
            "",
            &[
                SortKey {
                    column: Column::Transcript,
                    ascending: true,
                },
                SortKey {
                    column: Column::Recorded,
                    ascending: true,
                },
            ],
            date,
        );
        assert_eq!(ids(&r), ["09.15.00", "18.22.40", "11.05.09"]);
    }

    #[test]
    fn filters() {
        let (takes, docs) = library();
        let f = |filter| ids(&rows(&takes, &docs, filter, "", &[], date)).len();
        assert_eq!(f(Filter::Video), 1);
        assert_eq!(f(Filter::Transcribed), 1);
        assert_eq!(f(Filter::Untranscribed), 2);
        assert_eq!(f(Filter::TwoSpeaker), 0);
    }

    #[test]
    fn search_matches_names_ids_dates_and_transcripts() {
        let (takes, docs) = library();
        let s = |q| {
            ids(&rows(&takes, &docs, Filter::All, q, &[], date))
                .to_vec()
                .into_iter()
                .map(String::from)
                .collect::<Vec<_>>()
        };
        assert_eq!(s("guest"), ["11.05.09"]);
        assert_eq!(s("  HOST "), ["11.05.09", "18.22.40"]);
        assert_eq!(s("18.22"), ["18.22.40"]);
        assert_eq!(s("day 100"), ["09.15.00"]);
        assert_eq!(
            s("hidden line"),
            ["11.05.09"],
            "the whole transcript, not just the snippet"
        );
        assert!(s("nothing like this").is_empty());
    }

    #[test]
    fn snippets_and_text() {
        let (takes, docs) = library();
        let d = &docs[&takes[0].id];
        assert_eq!(
            snippet(Some(d)),
            "Welcome back. Thanks for having me. So, lavs."
        );
        assert_eq!(snippet(None), "");
        let text = transcript_text(d);
        assert!(text.starts_with("[00:00:00] Host: Welcome back.\n[00:00:07] Guest: Thanks"));
        let mut solo = d.clone();
        solo.speakers.truncate(1);
        assert!(transcript_text(&solo).starts_with("[00:00:00] Welcome back."));
    }

    #[test]
    fn copies_oldest_first_under_dates() {
        let (takes, mut docs) = library();
        docs.insert(takes[2].id.clone(), doc(&[("tx1", "Earlier.")]));
        let chosen: Vec<&Take> = takes.iter().collect();
        let text = copied_transcripts(&chosen, &docs, date).unwrap();
        assert!(text.starts_with("day 100\n\n[00:00:00] Host: Earlier.\n\nday 300\n\n"));
        let one = copied_transcripts(&chosen[..1], &docs, date).unwrap();
        assert!(one.starts_with("[00:00:00] Host: Welcome"));
        assert!(copied_transcripts(&chosen[1..2], &docs, date).is_none());
    }

    #[test]
    fn selection_after_delete() {
        let v: Vec<String> = ["a", "b", "c", "d"].iter().map(|s| s.to_string()).collect();
        let s = |d: &[&str]| {
            next_after_delete(&v, &d.iter().map(|s| s.to_string()).collect::<Vec<_>>())
        };
        assert_eq!(s(&["b"]).as_deref(), Some("c"));
        assert_eq!(s(&["b", "c"]).as_deref(), Some("d"));
        assert_eq!(s(&["d"]).as_deref(), Some("c"));
        assert_eq!(s(&["a", "b", "c", "d"]), None);
        assert_eq!(s(&["x"]), None);
    }

    #[test]
    fn summary() {
        let (takes, _) = library();
        let all: Vec<&Take> = takes.iter().collect();
        let s = summarize(&all);
        assert_eq!(
            (s.count, s.transcribed, s.with_video, s.files),
            (3, 1, 1, 5)
        );
        assert_eq!(s.span, Some((100.0, 300.0)));
        assert_eq!(s.total, 842.0 + 95.0 + 2650.0);
    }
}
