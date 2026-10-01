//! Input profile: what the three mapped fields of a source file look like,
//! and whether they look like what the models were trained on.
//!
//! [`InputProfiler`] is a pure accumulator fed one record at a time by
//! `preflight::validate` (dry run, shown in the import dialog) and by the
//! import worker (persisted on the dataset). It computes the per-column value
//! distributions, length and character-shape summaries per field, a fixed set
//! of row and dataset checks, and a handful of assembled model-input samples.
//! Checks that cross their threshold become [`Finding`]s. Findings inform;
//! they never block an import or a run (decision 2026-09-30). The thresholds
//! are set from a 39-institution reference panel on which no legitimate file
//! exceeded 1.2% on any warning check except short titles; see
//! `docs/input-contract.md`.
//!
//! Nothing here touches the assembled string or `content_hash`: the profiler
//! reads the same trimmed cells the import does and reports on them.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::{
    format::{CourseInput, format_input},
    preflight::{Samples, display, truncate_to},
};

pub(crate) const PROFILE_VERSION: u32 = 1;

/// Assembled model-input examples kept per profile.
const SAMPLE_COUNT: usize = 8;
/// Fixed seed so the same file yields the same samples every run.
const SAMPLE_SEED: u64 = 0x9E37_79B9_7F4A_7C15;
/// Distinct character-class shapes tracked per field; later shapes are
/// dropped from the map but still counted in the length histogram.
const SHAPE_CAP: usize = 64;
/// Shapes reported per field.
const TOP_SHAPES: usize = 8;
/// Length histogram buckets: index = min(chars, `LENGTH_BUCKETS` - 1).
const LENGTH_BUCKETS: usize = 257;

// Thresholds. Rates are over `importable` rows.
const WARN_RATE: f64 = 0.05;
const COERCED_RATE: f64 = 0.01;
const TITLE_SHORT_RATE: f64 = 0.10;
const CASE_NOTE_RATE: f64 = 0.50;
const WHITESPACE_NOTE_RATE: f64 = 0.01;
/// Any single damaged value is worth a note.
const ANY_RATE: f64 = 0.0;
const CARDINALITY_MIN_ROWS: u64 = 1_000;
const LOW_CARDINALITY_MAX: u64 = 5;
const HIGH_CARDINALITY_RATIO: f64 = 0.5;

/// Distinct values tracked per mapped column. Past the cap, known values keep
/// counting and new ones are dropped (`distinct_capped`), bounding memory on
/// 2M-row files where nearly every title is unique.
const DISTINCT_CAP: usize = 100_000;
/// Bytes of a value used as its distribution key, so the cap above bounds
/// memory at ~`DISTINCT_CAP * STAT_KEY_BYTES` per column even for hostile cells.
const STAT_KEY_BYTES: usize = 256;
/// Most frequent values reported per mapped column.
const TOP_VALUES: usize = 10;

// ---------------------------------------------------------------------------
// Output types
// ---------------------------------------------------------------------------

#[derive(Type, Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InputProfile {
    pub version: u32,
    /// Data records seen, excluding the header.
    pub rows: u64,
    /// Rows with subject, catalog, and title all non-empty after trim.
    pub importable: u64,
    /// Rows with at least one empty required field, by which field was empty.
    /// A row missing two fields counts in both.
    pub skipped: SkipCounts,
    pub columns: MappedColumns,
    pub fields: FieldShapes,
    /// Every check's raw count, whether or not it crossed its threshold.
    pub checks: Vec<CheckCount>,
    /// Checks that crossed their threshold, warnings first.
    pub findings: Vec<Finding>,
    /// Assembled model-input strings, spread across the file, distinct.
    pub samples: Vec<Sample>,
}

#[derive(Type, Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SkipCounts {
    pub total: u64,
    pub subject: u64,
    pub catalog: u64,
    pub title: u64,
}

#[derive(Type, Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MappedColumns {
    pub subject: ColumnStats,
    pub catalog: ColumnStats,
    pub title: ColumnStats,
}

#[derive(Type, Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ColumnStats {
    pub header: String,
    pub empty: u64,
    pub distinct: u64,
    /// True when [`DISTINCT_CAP`] was hit: `distinct` is a lower bound and
    /// `top` only reflects values seen before the cap.
    pub distinct_capped: bool,
    pub top: Vec<ValueCount>,
}

#[derive(Type, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ValueCount {
    pub value: String,
    pub count: u64,
}

#[derive(Type, Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FieldShapes {
    pub subject: FieldShape,
    pub catalog: FieldShape,
    pub title: FieldShape,
}

/// Length and character-shape summary of one field over importable rows.
#[derive(Type, Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FieldShape {
    /// Char counts over non-empty values.
    pub len_min: u32,
    pub len_median: u32,
    pub len_max: u32,
    /// Collapsed character-class shapes, most common first, at most 8.
    pub top_shapes: Vec<ValueCount>,
}

#[derive(Type, Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CheckCount {
    pub code: FindingCode,
    pub count: u64,
}

#[derive(Type, Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Finding {
    pub code: FindingCode,
    pub severity: Severity,
    pub field: Field,
    pub count: u64,
    /// `count / importable`; 0.0 for dataset-level checks with no row count.
    pub rate: f64,
    pub examples: Samples<Sample>,
}

#[derive(Type, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Sample {
    pub row: u64,
    pub input: String,
}

#[derive(Type, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Severity {
    Warning,
    Info,
}

#[derive(Type, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Field {
    Subject,
    Catalog,
    Title,
    Row,
}

/// Declared in report order: warnings, then notes. `checks` and `findings`
/// follow this order.
#[derive(Type, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FindingCode {
    CatalogHasSubjectPrefix,
    SubjectHasNumber,
    CatalogNumericCoerced,
    CatalogNoDigits,
    CatalogHasWhitespace,
    CatalogTooLong,
    CatalogLowCardinality,
    SubjectTooLong,
    SubjectHighCardinality,
    TitleNoLetters,
    TitleTooLong,
    TitleRepeatsCode,
    TitleShort,
    CatalogShort,
    SubjectLowercase,
    TitleMixedCase,
    EncodingDamage,
    InnerWhitespaceRuns,
}

impl FindingCode {
    const ALL: [Self; 18] = [
        Self::CatalogHasSubjectPrefix,
        Self::SubjectHasNumber,
        Self::CatalogNumericCoerced,
        Self::CatalogNoDigits,
        Self::CatalogHasWhitespace,
        Self::CatalogTooLong,
        Self::CatalogLowCardinality,
        Self::SubjectTooLong,
        Self::SubjectHighCardinality,
        Self::TitleNoLetters,
        Self::TitleTooLong,
        Self::TitleRepeatsCode,
        Self::TitleShort,
        Self::CatalogShort,
        Self::SubjectLowercase,
        Self::TitleMixedCase,
        Self::EncodingDamage,
        Self::InnerWhitespaceRuns,
    ];

    fn severity(self) -> Severity {
        match self {
            Self::CatalogHasSubjectPrefix
            | Self::SubjectHasNumber
            | Self::CatalogNumericCoerced
            | Self::CatalogNoDigits
            | Self::CatalogHasWhitespace
            | Self::CatalogTooLong
            | Self::CatalogLowCardinality
            | Self::SubjectTooLong
            | Self::SubjectHighCardinality
            | Self::TitleNoLetters
            | Self::TitleTooLong
            | Self::TitleRepeatsCode
            | Self::TitleShort => Severity::Warning,
            Self::CatalogShort
            | Self::SubjectLowercase
            | Self::TitleMixedCase
            | Self::EncodingDamage
            | Self::InnerWhitespaceRuns => Severity::Info,
        }
    }

    fn field(self) -> Field {
        match self {
            Self::SubjectHasNumber
            | Self::SubjectTooLong
            | Self::SubjectHighCardinality
            | Self::SubjectLowercase => Field::Subject,
            Self::CatalogHasSubjectPrefix
            | Self::CatalogNumericCoerced
            | Self::CatalogNoDigits
            | Self::CatalogHasWhitespace
            | Self::CatalogTooLong
            | Self::CatalogLowCardinality
            | Self::CatalogShort => Field::Catalog,
            Self::TitleNoLetters
            | Self::TitleTooLong
            | Self::TitleRepeatsCode
            | Self::TitleShort
            | Self::TitleMixedCase => Field::Title,
            Self::EncodingDamage | Self::InnerWhitespaceRuns => Field::Row,
        }
    }

    /// Share of importable rows at which a row check becomes a finding.
    /// Dataset checks (`None`) are decided in [`InputProfiler::finish`].
    fn threshold(self) -> Option<f64> {
        match self {
            Self::CatalogLowCardinality | Self::SubjectHighCardinality => None,
            Self::CatalogNumericCoerced => Some(COERCED_RATE),
            Self::TitleShort => Some(TITLE_SHORT_RATE),
            Self::SubjectLowercase | Self::TitleMixedCase => Some(CASE_NOTE_RATE),
            Self::EncodingDamage => Some(ANY_RATE),
            Self::InnerWhitespaceRuns => Some(WHITESPACE_NOTE_RATE),
            _ => Some(WARN_RATE),
        }
    }
}

// ---------------------------------------------------------------------------
// Row checks
// ---------------------------------------------------------------------------

fn char_len(s: &str) -> usize {
    s.chars().count()
}

/// `catalog` starts with `subject` (ASCII case-insensitive) and is longer; or
/// starts with two or more ASCII letters, an optional space or hyphen, and a
/// digit (`PSYC4325`, `PSYC 4325`, `CE-101`).
fn catalog_has_subject_prefix(subject: &str, catalog: &str) -> bool {
    let repeats_subject = char_len(subject) >= 2
        && catalog.len() > subject.len()
        && catalog
            .get(..subject.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(subject));
    repeats_subject || letters_then_digit(catalog)
}

fn letters_then_digit(s: &str) -> bool {
    let mut chars = s.chars().peekable();
    let mut letters = 0;
    while chars.next_if(char::is_ascii_alphabetic).is_some() {
        letters += 1;
    }
    if letters < 2 {
        return false;
    }
    chars.next_if(|c| *c == ' ' || *c == '-');
    chars.peek().is_some_and(char::is_ascii_digit)
}

fn two_consecutive_digits(s: &str) -> bool {
    s.as_bytes()
        .windows(2)
        .any(|w| w.iter().all(u8::is_ascii_digit))
}

/// `^[0-9]+\.0+$` (a spreadsheet float) or `^[0-9]+(\.[0-9]+)?[eE][+-]?[0-9]+$`
/// (scientific notation): the shapes a numeric column takes after a tool
/// coerced it. `101.5` and `4304L` are not these.
fn numeric_coerced(s: &str) -> bool {
    let mut chars = s.chars().peekable();
    let mut int_digits = 0;
    while chars.next_if(char::is_ascii_digit).is_some() {
        int_digits += 1;
    }
    if int_digits == 0 {
        return false;
    }
    let mut frac_zero_only = true;
    let mut frac_digits = 0;
    if chars.next_if_eq(&'.').is_some() {
        while let Some(d) = chars.next_if(char::is_ascii_digit) {
            frac_digits += 1;
            frac_zero_only &= d == '0';
        }
        if frac_digits == 0 {
            return false;
        }
    }
    match chars.next() {
        None => frac_digits > 0 && frac_zero_only,
        Some('e' | 'E') => {
            chars.next_if(|c| *c == '+' || *c == '-');
            let mut exp_digits = 0;
            while chars.next_if(char::is_ascii_digit).is_some() {
                exp_digits += 1;
            }
            exp_digits > 0 && chars.next().is_none()
        }
        Some(_) => false,
    }
}

/// `title` starts with `"{subject} {catalog}"`, ASCII case-insensitive.
fn title_repeats_code(subject: &str, catalog: &str, title: &str) -> bool {
    let Some(after_subject) = title
        .get(..subject.len())
        .filter(|head| head.eq_ignore_ascii_case(subject))
        .and_then(|_| title.get(subject.len()..))
    else {
        return false;
    };
    let Some(after_space) = after_subject.strip_prefix(' ') else {
        return false;
    };
    after_space
        .get(..catalog.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(catalog))
}

fn inner_whitespace_run(s: &str) -> bool {
    s.contains("  ") || s.contains(['\t', '\r', '\n'])
}

/// Which row checks fire for one importable row, in [`FindingCode::ALL`] order.
fn row_hits(subject: &str, catalog: &str, title: &str) -> impl Iterator<Item = FindingCode> {
    let any = |pred: fn(char) -> bool| {
        subject.chars().any(pred) || catalog.chars().any(pred) || title.chars().any(pred)
    };
    let hits = [
        (
            FindingCode::CatalogHasSubjectPrefix,
            catalog_has_subject_prefix(subject, catalog),
        ),
        (
            FindingCode::SubjectHasNumber,
            two_consecutive_digits(subject),
        ),
        (FindingCode::CatalogNumericCoerced, numeric_coerced(catalog)),
        (
            FindingCode::CatalogNoDigits,
            !catalog.chars().any(|c| c.is_ascii_digit()),
        ),
        (
            FindingCode::CatalogHasWhitespace,
            catalog.chars().any(char::is_whitespace),
        ),
        (FindingCode::CatalogTooLong, char_len(catalog) > 8),
        (FindingCode::SubjectTooLong, char_len(subject) > 8),
        (
            FindingCode::TitleNoLetters,
            !title.chars().any(char::is_alphabetic),
        ),
        (FindingCode::TitleTooLong, char_len(title) > 100),
        (
            FindingCode::TitleRepeatsCode,
            title_repeats_code(subject, catalog, title),
        ),
        (FindingCode::TitleShort, char_len(title) <= 3),
        (FindingCode::CatalogShort, char_len(catalog) <= 2),
        (
            FindingCode::SubjectLowercase,
            subject.chars().any(char::is_lowercase),
        ),
        (
            FindingCode::TitleMixedCase,
            title.chars().any(char::is_lowercase),
        ),
        (FindingCode::EncodingDamage, any(|c| c == '\u{FFFD}')),
        (
            FindingCode::InnerWhitespaceRuns,
            inner_whitespace_run(subject)
                || inner_whitespace_run(catalog)
                || inner_whitespace_run(title),
        ),
    ];
    hits.into_iter()
        .filter_map(|(code, hit)| hit.then_some(code))
}

// ---------------------------------------------------------------------------
// Accumulators
// ---------------------------------------------------------------------------

/// Per-column value distribution: empties, distinct count, top values.
pub(crate) struct ColumnTally {
    header: String,
    empty: u64,
    counts: HashMap<String, u64>,
    capped: bool,
}

impl ColumnTally {
    fn new(header: String) -> Self {
        Self {
            header,
            empty: 0,
            counts: HashMap::new(),
            capped: false,
        }
    }

    fn observe(&mut self, value: &str) {
        if value.is_empty() {
            self.empty += 1;
            return;
        }
        let key = truncate_to(value.to_owned(), STAT_KEY_BYTES);
        if let Some(n) = self.counts.get_mut(&key) {
            *n += 1;
        } else if self.counts.len() < DISTINCT_CAP {
            self.counts.insert(key, 1);
        } else {
            self.capped = true;
        }
    }

    /// Distinct non-empty values seen; a capped tally reports its cap.
    fn distinct(&self) -> u64 {
        self.counts.len() as u64
    }

    fn finish(self) -> ColumnStats {
        let distinct = self.distinct();
        ColumnStats {
            header: self.header,
            empty: self.empty,
            distinct,
            distinct_capped: self.capped,
            top: top_values(self.counts, TOP_VALUES),
        }
    }
}

fn top_values(counts: HashMap<String, u64>, limit: usize) -> Vec<ValueCount> {
    let mut top: Vec<ValueCount> = counts
        .into_iter()
        .map(|(value, count)| ValueCount {
            value: display(&value),
            count,
        })
        .collect();
    top.sort_unstable_by(|a, b| b.count.cmp(&a.count).then_with(|| a.value.cmp(&b.value)));
    top.truncate(limit);
    top
}

/// Map each char to its class and collapse runs: `PSYC` -> `A+`, `4304L` ->
/// `9+A`, `B A` -> `A_A`, `101.0` -> `9+.9`. Output stops at
/// [`STAT_KEY_BYTES`]: a hostile cell alternating classes would otherwise
/// yield a shape as long as itself, and shapes are held until `finish`.
fn shape_of(value: &str) -> String {
    let mut out = String::new();
    let mut run: Option<(char, bool)> = None;
    let flush = |out: &mut String, run: Option<(char, bool)>| {
        if let Some((c, repeated)) = run {
            out.push(c);
            if repeated {
                out.push('+');
            }
        }
    };
    for c in value.chars() {
        if out.len() >= STAT_KEY_BYTES {
            return out;
        }
        let class = if c.is_ascii_digit() {
            '9'
        } else if c.is_uppercase() {
            'A'
        } else if c.is_lowercase() {
            'a'
        } else if c.is_whitespace() {
            '_'
        } else {
            c
        };
        match run {
            Some((prev, _)) if prev == class => run = Some((class, true)),
            _ => {
                flush(&mut out, run);
                run = Some((class, false));
            }
        }
    }
    flush(&mut out, run);
    out
}

struct ShapeTally {
    lengths: Box<[u64; LENGTH_BUCKETS]>,
    shapes: HashMap<String, u64>,
}

impl ShapeTally {
    fn new() -> Self {
        Self {
            lengths: Box::new([0; LENGTH_BUCKETS]),
            shapes: HashMap::new(),
        }
    }

    fn observe(&mut self, value: &str) {
        let bucket = char_len(value).min(LENGTH_BUCKETS - 1);
        if let Some(n) = self.lengths.get_mut(bucket) {
            *n += 1;
        }
        let shape = shape_of(value);
        if let Some(n) = self.shapes.get_mut(&shape) {
            *n += 1;
        } else if self.shapes.len() < SHAPE_CAP {
            self.shapes.insert(shape, 1);
        }
    }

    fn finish(self) -> FieldShape {
        let total: u64 = self.lengths.iter().sum();
        let nonzero = || self.lengths.iter().enumerate().filter(|(_, n)| **n > 0);
        let len_min = nonzero().next().map_or(0, |(i, _)| i);
        let len_max = nonzero().next_back().map_or(0, |(i, _)| i);
        // Lower median: the value at sorted position (total - 1) / 2.
        let target = total.saturating_sub(1) / 2;
        let mut seen = 0;
        let mut len_median = 0;
        for (i, n) in nonzero() {
            seen += n;
            if seen > target {
                len_median = i;
                break;
            }
        }
        // Bucket indexes are below LENGTH_BUCKETS, so the conversion cannot fail.
        let bucket = |i: usize| u32::try_from(i).unwrap_or(u32::MAX);
        FieldShape {
            len_min: bucket(len_min),
            len_median: bucket(len_median),
            len_max: bucket(len_max),
            top_shapes: top_values(self.shapes, TOP_SHAPES),
        }
    }
}

#[expect(clippy::cast_precision_loss, reason = "row counts won't approach 2^52")]
fn rate(count: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        count as f64 / total as f64
    }
}

struct CheckTally {
    code: FindingCode,
    examples: Samples<Sample>,
}

/// Reservoir of distinct assembled inputs spread across the file, with a
/// fixed-seed xorshift64 so the same file yields the same samples.
struct Reservoir {
    slots: Vec<Sample>,
    seen: u64,
    state: u64,
}

impl Reservoir {
    fn new() -> Self {
        Self {
            slots: Vec::with_capacity(SAMPLE_COUNT),
            seen: 0,
            state: SAMPLE_SEED,
        }
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    /// Algorithm R; the candidate is only assembled when it wins a slot, and
    /// is dropped if an identical input is already held.
    fn offer(&mut self, row: u64, input: impl FnOnce() -> String) {
        self.seen += 1;
        let slot = if self.slots.len() < SAMPLE_COUNT {
            self.slots.len()
        } else {
            let j = self.next_u64() % self.seen;
            match usize::try_from(j) {
                Ok(j) if j < SAMPLE_COUNT => j,
                _ => return,
            }
        };
        let input = input();
        if self.slots.iter().any(|s| s.input == input) {
            return;
        }
        let sample = Sample { row, input };
        match self.slots.get_mut(slot) {
            Some(existing) => *existing = sample,
            None => self.slots.push(sample),
        }
    }

    fn finish(mut self) -> Vec<Sample> {
        self.slots.sort_unstable_by_key(|s| s.row);
        self.slots
    }
}

/// Accumulates an [`InputProfile`] one record at a time.
pub(crate) struct InputProfiler {
    tallies: [ColumnTally; 3],
    shapes: [ShapeTally; 3],
    checks: Vec<CheckTally>,
    reservoir: Reservoir,
    rows: u64,
    importable: u64,
    skipped: SkipCounts,
}

impl InputProfiler {
    /// `headers` are the mapped columns' header text: subject, catalog, title.
    pub(crate) fn new(headers: [String; 3]) -> Self {
        Self {
            tallies: headers.map(ColumnTally::new),
            shapes: [ShapeTally::new(), ShapeTally::new(), ShapeTally::new()],
            checks: FindingCode::ALL
                .into_iter()
                .map(|code| CheckTally {
                    code,
                    examples: Samples::new(),
                })
                .collect(),
            reservoir: Reservoir::new(),
            rows: 0,
            importable: 0,
            skipped: SkipCounts::default(),
        }
    }

    /// Called once per data record with the trimmed mapped cells. `row` is
    /// the spreadsheet row. Tallies and skip counts see every record; shapes,
    /// checks, and samples only see importable rows.
    pub(crate) fn observe(&mut self, row: u64, cells: [&str; 3]) {
        self.rows += 1;
        let [subject, catalog, title] = cells;
        for (tally, cell) in self.tallies.iter_mut().zip(cells) {
            tally.observe(cell);
        }
        if cells.iter().any(|c| c.is_empty()) {
            self.skipped.total += 1;
            self.skipped.subject += u64::from(subject.is_empty());
            self.skipped.catalog += u64::from(catalog.is_empty());
            self.skipped.title += u64::from(title.is_empty());
            return;
        }
        self.importable += 1;
        for (shape, cell) in self.shapes.iter_mut().zip(cells) {
            shape.observe(cell);
        }
        let input = || {
            display(&format_input(&CourseInput {
                subject_code: subject.to_owned(),
                catalog_number: catalog.to_owned(),
                course_title: title.to_owned(),
            }))
        };
        for code in row_hits(subject, catalog, title) {
            if let Some(check) = self.checks.iter_mut().find(|c| c.code == code) {
                check.examples.push(|| Sample {
                    row,
                    input: input(),
                });
            }
        }
        self.reservoir.offer(row, input);
    }

    pub(crate) fn finish(self) -> InputProfile {
        let Self {
            tallies,
            shapes,
            checks,
            reservoir,
            rows,
            importable,
            skipped,
        } = self;
        let [subject_tally, catalog_tally, title_tally] = tallies;
        let [subject_shape, catalog_shape, title_shape] = shapes;

        let dataset_checks = importable >= CARDINALITY_MIN_ROWS;
        let catalog_distinct = catalog_tally.distinct();
        let subject_distinct = subject_tally.distinct();
        let low_catalog = dataset_checks && catalog_distinct <= LOW_CARDINALITY_MAX;
        let high_subject =
            dataset_checks && rate(subject_distinct, importable) > HIGH_CARDINALITY_RATIO;

        let mut check_counts = Vec::with_capacity(checks.len());
        let mut findings = Vec::new();
        for check in checks {
            let code = check.code;
            let (count, rate, examples, fired) = if let Some(threshold) = code.threshold() {
                let count = check.examples.count;
                let rate = rate(count, importable);
                (count, rate, check.examples, count > 0 && rate >= threshold)
            } else {
                let distinct = match code {
                    FindingCode::CatalogLowCardinality => low_catalog.then_some(catalog_distinct),
                    FindingCode::SubjectHighCardinality => high_subject.then_some(subject_distinct),
                    _ => None,
                };
                (
                    distinct.unwrap_or(0),
                    0.0,
                    Samples::new(),
                    distinct.is_some(),
                )
            };
            check_counts.push(CheckCount { code, count });
            if fired {
                findings.push(Finding {
                    code,
                    severity: code.severity(),
                    field: code.field(),
                    count,
                    rate,
                    examples,
                });
            }
        }
        findings.sort_by_key(|f| f.severity == Severity::Info);

        InputProfile {
            version: PROFILE_VERSION,
            rows,
            importable,
            skipped,
            columns: MappedColumns {
                subject: subject_tally.finish(),
                catalog: catalog_tally.finish(),
                title: title_tally.finish(),
            },
            fields: FieldShapes {
                subject: subject_shape.finish(),
                catalog: catalog_shape.finish(),
                title: title_shape.finish(),
            },
            checks: check_counts,
            findings,
            samples: reservoir.finish(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(rows: &[[&str; 3]]) -> InputProfile {
        let mut p = InputProfiler::new(["sub_pref".into(), "course".into(), "title".into()]);
        for (i, cells) in rows.iter().enumerate() {
            p.observe(i as u64 + 2, *cells);
        }
        p.finish()
    }

    fn count(profile: &InputProfile, code: FindingCode) -> u64 {
        profile
            .checks
            .iter()
            .find(|c| c.code == code)
            .map_or(u64::MAX, |c| c.count)
    }

    fn finding(profile: &InputProfile, code: FindingCode) -> Option<&Finding> {
        profile.findings.iter().find(|f| f.code == code)
    }

    /// Which row checks a single row trips, in report order.
    fn hits(cells: [&str; 3]) -> Vec<FindingCode> {
        let p = profile(&[cells]);
        p.checks
            .iter()
            .filter(|c| c.count > 0)
            .map(|c| c.code)
            .collect()
    }

    #[test]
    fn row_checks_positive_and_negative() {
        use FindingCode::*;
        // Correct rows from the reference envelope fire nothing but the
        // legitimate notes.
        assert_eq!(hits(["PSYC", "4325", "ABNORMAL PSYCHOLOGY"]), vec![]);
        assert_eq!(hits(["CHEM", "318Q", "ORGANIC CHEM LAB"]), vec![]);
        assert_eq!(hits(["B A", "1301", "INTRO TO BUSINESS"]), vec![]);
        assert_eq!(hits(["BIOL", "4304L", "GENETICS LAB"]), vec![]);
        assert_eq!(hits(["CORE", "101", "CORE SEMINAR"]), vec![]);

        // A combined column trips three checks at once: the prefix, the
        // space, and (at nine characters) the length.
        assert_eq!(
            hits(["PSYC", "PSYC 4325", "ABNORMAL PSYCHOLOGY"]),
            vec![
                CatalogHasSubjectPrefix,
                CatalogHasWhitespace,
                CatalogTooLong
            ]
        );
        assert_eq!(
            hits(["ECON", "psyc4325", "T"]),
            vec![CatalogHasSubjectPrefix, TitleShort]
        );
        assert_eq!(
            hits(["CE", "CE-101", "STATICS"]),
            vec![CatalogHasSubjectPrefix]
        );
        assert_eq!(hits(["ECON 101", "101", "MICRO"]), vec![SubjectHasNumber]);
        assert_eq!(hits(["ECON", "ABC", "MICRO"]), vec![CatalogNoDigits]);
        assert_eq!(hits(["ECON", "123456789", "MICRO"]), vec![CatalogTooLong]);
        assert_eq!(hits(["ECONOMICS", "101", "MICRO"]), vec![SubjectTooLong]);
        assert_eq!(hits(["ECON", "101", "12345"]), vec![TitleNoLetters]);
        assert_eq!(hits(["ECON", "101", &"X".repeat(101)]), vec![TitleTooLong]);
        assert_eq!(
            hits(["ECON", "101", "Econ 101 Principles of Micro"]),
            vec![TitleRepeatsCode, TitleMixedCase]
        );
        assert_eq!(hits(["ECON", "101", "ESL"]), vec![TitleShort]);
        assert_eq!(hits(["ECON", "1", "MICRO"]), vec![CatalogShort]);
        assert_eq!(hits(["econ", "101", "MICRO"]), vec![SubjectLowercase]);
        assert_eq!(
            hits(["ECON", "101", "MICRO \u{FFFD}"]),
            vec![EncodingDamage]
        );
        assert_eq!(
            hits(["ECON", "101", "MICRO  ECON"]),
            vec![InnerWhitespaceRuns]
        );
        assert_eq!(
            hits(["ECON", "101", "MICRO\tECON"]),
            vec![InnerWhitespaceRuns]
        );
    }

    #[test]
    fn numeric_coercion_shapes() {
        for s in ["101.0", "101.00", "1e3", "1.01E+02"] {
            assert!(numeric_coerced(s), "{s}");
        }
        for s in ["101", "101.5", "4304L", "0", "1e", ".0", "101."] {
            assert!(!numeric_coerced(s), "{s}");
        }
    }

    #[test]
    fn shapes_collapse_runs() {
        assert_eq!(shape_of("PSYC"), "A+");
        assert_eq!(shape_of("4304L"), "9+A");
        assert_eq!(shape_of("B A"), "A_A");
        assert_eq!(shape_of("101.0"), "9+.9");
        assert_eq!(shape_of("Intro"), "Aa+");
        // Alternating classes never collapse; the shape is cut, not the cell.
        let hostile: String = "a1".repeat(10_000);
        assert!(shape_of(&hostile).len() <= STAT_KEY_BYTES + 1);
    }

    #[test]
    fn threshold_is_five_percent_of_importable() -> Result<(), String> {
        let bad = ["PSYC", "PSYC 4325", "ABNORMAL"];
        let good = ["PSYC", "4325", "ABNORMAL"];
        let mut rows = vec![bad; 4];
        rows.extend(std::iter::repeat_n(good, 96));
        let p = profile(&rows);
        assert_eq!(count(&p, FindingCode::CatalogHasSubjectPrefix), 4);
        assert!(finding(&p, FindingCode::CatalogHasSubjectPrefix).is_none());

        let mut rows = vec![bad; 5];
        rows.extend(std::iter::repeat_n(good, 95));
        let p = profile(&rows);
        let f = finding(&p, FindingCode::CatalogHasSubjectPrefix).ok_or("no finding")?;
        assert_eq!(
            (f.count, f.severity, f.field),
            (5, Severity::Warning, Field::Catalog)
        );
        assert!((f.rate - 0.05).abs() < f64::EPSILON);
        assert_eq!(f.examples.count, 5);
        assert_eq!(
            f.examples.first.first().map(|s| s.input.as_str()),
            Some("PSYC PSYC 4325 --- ABNORMAL")
        );
        Ok(())
    }

    #[test]
    fn one_damaged_value_is_a_note() -> Result<(), String> {
        let mut rows = vec![["ECON", "101", "MICRO"]; 99];
        rows.push(["ECON", "102", "MACRO \u{FFFD}"]);
        let p = profile(&rows);
        let f = finding(&p, FindingCode::EncodingDamage).ok_or("no finding")?;
        assert_eq!(
            (f.count, f.severity, f.field),
            (1, Severity::Info, Field::Row)
        );
        Ok(())
    }

    #[test]
    fn cardinality_checks_need_a_thousand_rows() -> Result<(), String> {
        let catalogs = ["1", "2", "3"];
        let rows: Vec<[&str; 3]> = catalogs
            .iter()
            .cycle()
            .take(1000)
            .map(|c| ["ECON", c, "MICRO"])
            .collect();
        let p = profile(&rows);
        let f = finding(&p, FindingCode::CatalogLowCardinality).ok_or("no finding")?;
        assert_eq!((f.count, f.rate), (3, 0.0));
        assert_eq!(count(&p, FindingCode::CatalogLowCardinality), 3);

        let p = profile(rows.get(..999).ok_or("slice")?);
        assert!(finding(&p, FindingCode::CatalogLowCardinality).is_none());
        assert_eq!(count(&p, FindingCode::CatalogLowCardinality), 0);
        Ok(())
    }

    #[test]
    fn subject_high_cardinality() {
        let subjects: Vec<String> = (0..1000).map(|i| format!("S{i:04}")).collect();
        let rows: Vec<[&str; 3]> = subjects
            .iter()
            .map(|s| [s.as_str(), "101", "MICRO"])
            .collect();
        let p = profile(&rows);
        assert_eq!(
            finding(&p, FindingCode::SubjectHighCardinality).map(|f| f.count),
            Some(1000)
        );
    }

    #[test]
    fn skip_counts_per_missing_field() {
        let p = profile(&[
            ["", "101", ""],
            ["ECON", "", "MICRO"],
            ["ECON", "101", "MICRO"],
        ]);
        assert_eq!(p.rows, 3);
        assert_eq!(p.importable, 1);
        assert_eq!(
            (
                p.skipped.total,
                p.skipped.subject,
                p.skipped.catalog,
                p.skipped.title
            ),
            (2, 1, 1, 1)
        );
        assert_eq!(p.columns.subject.empty, 1);
    }

    #[test]
    fn field_shapes_summarize_lengths() {
        let p = profile(&[
            ["PSYC", "4325", "A"],
            ["B A", "318Q", "ABC"],
            ["ECON", "1", "ABCDE"],
        ]);
        let s = &p.fields.subject;
        assert_eq!((s.len_min, s.len_median, s.len_max), (3, 4, 4));
        assert_eq!(
            s.top_shapes,
            vec![
                ValueCount {
                    value: "A+".into(),
                    count: 2
                },
                ValueCount {
                    value: "A_A".into(),
                    count: 1
                },
            ]
        );
        let t = &p.fields.title;
        assert_eq!((t.len_min, t.len_median, t.len_max), (1, 3, 5));
    }

    #[test]
    fn samples_are_distinct_sorted_and_stable() {
        let titles: Vec<String> = (0..500).map(|i| format!("TITLE {}", i / 2)).collect();
        let rows: Vec<[&str; 3]> = titles.iter().map(|t| ["ECON", "101", t.as_str()]).collect();
        let a = profile(&rows);
        let b = profile(&rows);
        assert_eq!(a.samples.len(), SAMPLE_COUNT);
        assert_eq!(a.samples, b.samples);
        let inputs: std::collections::HashSet<&str> =
            a.samples.iter().map(|s| s.input.as_str()).collect();
        assert_eq!(inputs.len(), SAMPLE_COUNT);
        assert!(
            a.samples
                .windows(2)
                .all(|w| matches!(w, [x, y] if x.row < y.row))
        );
    }

    #[test]
    fn serde_round_trip() -> Result<(), serde_json::Error> {
        let p = profile(&[["PSYC", "PSYC 4325", "ABNORMAL"], ["", "1", "X"]]);
        let json = serde_json::to_string(&p)?;
        let back: InputProfile = serde_json::from_str(&json)?;
        assert_eq!(serde_json::to_string(&back)?, json);
        assert!(json.contains("\"catalog_has_subject_prefix\""));
        assert!(json.contains("\"lenMedian\""));
        Ok(())
    }
}
