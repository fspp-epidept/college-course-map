# Model input contract

Audience: administrators preparing a CSV for classification, and developers
changing the import path. This document is the authoritative description of
what the app sends to the classification models and what it does to the
values on the way. It is versioned with the code that implements it
(`src-tauri/src/format.rs`, `src-tauri/src/preflight.rs`,
`src-tauri/src/import.rs`, `scripts/models/_lib/format.py`).

## The model input

Every course becomes one string:

    {subject code} {catalog number} --- {course title}

with one space between the subject code and the catalog number, and a
separator of space, three hyphens, space before the title. Example:

    ECON 101 --- Principles of Microeconomics

This is the format the models were trained on (annamp model cards, verified
2026-05). The template is `FORMAT_VERSION = "v1"`; Rust and Python share it
through `scripts/models/_lib/format_spec.json`, and a test fails if they
drift.

## The three fields

| Field | What it is | Examples | Not this |
| --- | --- | --- | --- |
| Subject code | The department or subject prefix as the registrar prints it | `ECON`, `PSYC`, `B A`, `CE-1` | A department name (`Economics`), a code with the number attached (`ECON101`) |
| Catalog number | The course number as printed, including letter suffixes and leading zeros | `101`, `4304L`, `001F`, `318Q` | A number that lost its formatting (`101.0`, `1` for `001`), the subject repeated (`ECON 101`), a year or credit count |
| Course title | The catalog title | `ABNORMAL PSYCHOLOGY`, `Intro. to Counseling Skills` | A description paragraph, the code repeated in the title (`PSYC 4325 ABNORMAL PSYCHOLOGY`), an empty cell |

All three are text. The app never converts them to numbers. Each must be
non-empty after trimming; a row missing any of the three is skipped and
counted, never classified.

## What correctly formatted data looks like

Measured on a reference panel of 1.9 million rows from 39 institutions
(2026-09-29). This is reference data, not the training set.

| Field | Length (characters) | Character shape |
| --- | --- | --- |
| Subject code | 1-7; 3-4 in 93% of rows | Uppercase letters only in 97%; a few with an inner space (`B A`), a period, hyphen, or ampersand |
| Catalog number | 1-7; 3-4 in 99% of rows | Digits only in 93%; digits plus a letter suffix (`318Q`, `679HA`) in 7% |
| Course title | 1-30 | Uppercase and abbreviated in this panel; registrar systems often truncate at 30 |

The training data (NCES transcript studies) is not available for inspection.
Whether its titles were uppercase or mixed case has not been confirmed.

## What the app does to a value

In order, and nothing else:

1. Reads the cell as text. Files are decoded as UTF-8, or as Windows-1252 or
   Mac Roman when the user confirms that in the import dialog.
2. Trims leading and trailing whitespace.
3. Skips the row if the subject, catalog number, or title is empty.
4. Truncates a value longer than 8 KiB.
5. Assembles the string above.
6. Hashes the assembled string (blake3) into `content_hash`, the key of the
   results cache. Two rows with the same three values share one
   classification.
7. Tokenizes with the model's tokenizer, truncated at 512 tokens.

The app does not change case, collapse inner whitespace, pad numbers, or
apply Unicode normalization. Damage done before the file reached the app,
such as Excel turning `001F` into `1` or `101` into `101.0`, cannot be
repaired; the import checks below try to notice it.

## Preparing a file

- Keep the subject code and catalog number in separate columns.
- Export catalog numbers as text so spreadsheets keep leading zeros and
  letter suffixes. In Excel, format the column as Text before pasting, or
  save from the source system directly as CSV.
- Save as "CSV UTF-8" when the tool offers it.
- Do not include the code in the title column.

## Stability rule

The assembled string is the cache key and the parity fixture input. Any
change to steps 2-5 above is a change to `FORMAT_VERSION`: it invalidates
every cached classification, it has to be mirrored in the Python formatter,
and parity has to be re-verified. Normalization options are a separate
decision (2026-09-30: none are applied).

## Checks the app runs

During import the app profiles every row and reports findings. Findings
inform; they never block an import or a classification run.

| Finding | Warns when | Why it matters |
| --- | --- | --- |
| Catalog number starts with the subject code | 5% of rows | The subject appears twice in the model input |
| Subject code contains a number | 5% | The subject column probably holds the full course code |
| Catalog number looks like a converted number (`101.0`, `1e3`) | 1% | A spreadsheet reformatted the column; leading zeros and suffixes are probably gone too |
| Catalog number has no digits | 5% | The wrong column may be mapped |
| Catalog number contains whitespace | 5% | Usually a combined or wrong column |
| Catalog number longer than 8 characters | 5% | Usually a combined or wrong column |
| Catalog number has 5 or fewer distinct values | dataset | A year, term, or credit column is mapped as the catalog number |
| Subject code longer than 8 characters | 5% | Department names instead of codes |
| Subject code has almost as many distinct values as rows | dataset | An identifier column is mapped as the subject |
| Title has no letters | 5% | The wrong column may be mapped |
| Title longer than 100 characters | 5% | A description column is mapped as the title |
| Title starts with the subject and catalog number | 5% | The code appears twice in the model input |
| Title is 3 characters or shorter | 10% | Titles this short carry little signal |
| Catalog number is 2 characters or shorter (note) | 5% | Possible lost leading zeros |
| Lowercase in the subject code or title (note) | 50% | Differs from the reference data; see the sensitivity table |
| Replacement character U+FFFD present (note) | any row | Text was damaged by an earlier encoding conversion |
| Runs of inner whitespace, tabs, or line breaks (note) | 1% | Usually an export artifact |

Legitimate data stays below these thresholds: on the reference panel no
institution exceeded 1.2% on any warning check except short titles (2.9%).

## Measured sensitivity

How much predictions change when correctly formatted inputs are altered
(5,000 distinct reference inputs, ModernBERT models, 2026-10-01; the CIP
column is a proxy for correctness, not model accuracy; see
`scripts/models/sensitivity.py` and
`scripts/models/reports/sensitivity-latest.md` for the full table).

| Alteration | 2-digit predictions changed | 6-digit predictions changed |
| --- | --- | --- |
| Subject code missing | 35% | 40% |
| Title missing | 30% | 78% |
| Title placed in the catalog column | 18% | 28% |
| Subject and catalog number swapped | 17% | 28% |
| Title lowercased | 16% | 36% |
| Subject code lowercased | 15% | 26% |
| Title in Title Case | 15% | 32% |
| Catalog number missing | 14% | 27% |
| Subject repeated in the catalog number | 10% | 20% |
| Catalog number as `4325.0` | 9% | 17% |

The models' confidence does not drop on altered input, so these problems
cannot be detected from the results. They have to be caught in the file.
