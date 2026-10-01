import type { FindingCode } from "../bindings";

// User-facing copy for every input-profile check (src-tauri/src/profile.rs).
// `Record<FindingCode, ...>` makes vue-tsc fail when a code has no copy, so a
// new check in Rust cannot ship without text. Thresholds and the reasoning
// behind each check live in docs/input-contract.md.
export interface FindingCopy {
  title: string;
  explanation: string;
  remedy: string;
}

export const INPUT_FINDINGS: Record<FindingCode, FindingCopy> = {
  catalog_has_subject_prefix: {
    title: "Catalog numbers start with the subject code",
    explanation:
      "The subject code would appear twice in what the model reads, for example PSYC PSYC 4325.",
    remedy:
      "Map a column that holds only the number, or split the combined column in the source file.",
  },
  subject_has_number: {
    title: "Subject codes contain course numbers",
    explanation: "The subject column looks like it holds the whole course code.",
    remedy: "Map a column that holds only the subject prefix.",
  },
  catalog_numeric_coerced: {
    title: "Catalog numbers look like converted numbers",
    explanation:
      "Values such as 101.0 mean a spreadsheet treated the column as numeric; leading zeros and letter suffixes are usually lost as well.",
    remedy: "Re-export the column as text from the source system.",
  },
  catalog_no_digits: {
    title: "Catalog numbers have no digits",
    explanation: "Course numbers almost always contain digits; this column may be the wrong one.",
    remedy: "Check the column mapping.",
  },
  catalog_has_whitespace: {
    title: "Catalog numbers contain spaces",
    explanation: "A catalog number with spaces is usually a combined or wrong column.",
    remedy: "Check the column mapping.",
  },
  catalog_too_long: {
    title: "Catalog numbers are unusually long",
    explanation: "Course numbers are rarely longer than 8 characters.",
    remedy: "Check the column mapping.",
  },
  catalog_low_cardinality: {
    title: "Almost every row has the same catalog number",
    explanation: "A year, term, or credit column may be mapped as the catalog number.",
    remedy: "Check the column mapping.",
  },
  subject_too_long: {
    title: "Subject codes are unusually long",
    explanation: "Department names instead of codes change what the model reads.",
    remedy: "Map the code column, not the name column.",
  },
  subject_high_cardinality: {
    title: "Subject codes are almost all different",
    explanation: "An identifier column may be mapped as the subject.",
    remedy: "Check the column mapping.",
  },
  title_no_letters: {
    title: "Titles have no letters",
    explanation: "This column may be the wrong one.",
    remedy: "Check the column mapping.",
  },
  title_too_long: {
    title: "Titles are unusually long",
    explanation: "A description column may be mapped as the title.",
    remedy: "Map the title column.",
  },
  title_repeats_code: {
    title: "Titles start with the course code",
    explanation: "The code would appear twice in what the model reads.",
    remedy: "Remove the code from the title column in the source file.",
  },
  title_short: {
    title: "Titles are very short",
    explanation: "Titles of three characters or fewer carry little information.",
    remedy: "Check that the full title column is mapped.",
  },
  catalog_short: {
    title: "Some catalog numbers are very short",
    explanation: "Values of one or two characters can mean leading zeros were lost.",
    remedy: "If the source has longer numbers, re-export the column as text.",
  },
  subject_lowercase: {
    title: "Subject codes contain lowercase letters",
    explanation: "The reference data is uppercase; predictions can differ for lowercase input.",
    remedy: "No action required; see the input contract.",
  },
  title_mixed_case: {
    title: "Titles contain lowercase letters",
    explanation: "The reference data is uppercase; predictions can differ for mixed-case input.",
    remedy: "No action required; see the input contract.",
  },
  encoding_damage: {
    title: "Some values contain the replacement character",
    explanation: "Text was damaged by an earlier encoding conversion.",
    remedy: "Re-export the file from the source system as UTF-8.",
  },
  inner_whitespace_runs: {
    title: "Some values contain repeated spaces or line breaks",
    explanation: "Usually an export artifact; the model reads them as-is.",
    remedy: "Clean the source column if possible.",
  },
};
