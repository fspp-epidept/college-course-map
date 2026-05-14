//! Canonical model-input formatter — Rust mirror of `scripts/models/_lib/format.py`.
//!
//! `TEMPLATE`, `FIELDS`, and `FORMAT_VERSION` are the contract; `assert_matches_spec`
//! reads the JSON the Python pipeline emits and panics on drift.

use std::path::Path;

use serde::Deserialize;

pub const FORMAT_VERSION: &str = "v1";
pub const TEMPLATE: &str = "{subject_code} {catalog_number} --- {course_title}";
pub const FIELDS: &[&str] = &["subject_code", "catalog_number", "course_title"];

#[derive(Debug, Clone)]
pub struct CourseInput {
    pub subject_code: String,
    pub catalog_number: String,
    pub course_title: String,
}

#[must_use]
pub fn format_input(course: &CourseInput) -> String {
    let CourseInput {
        subject_code,
        catalog_number,
        course_title,
    } = course;
    format!("{subject_code} {catalog_number} --- {course_title}")
}

#[derive(Debug, Deserialize)]
struct FormatSpec {
    version: String,
    template: String,
    fields: Vec<String>,
}

pub fn assert_matches_spec(spec_path: &Path) -> anyhow::Result<()> {
    let display = spec_path.display();
    let bytes = std::fs::read(spec_path).map_err(|e| anyhow::anyhow!("read {display}: {e}"))?;
    let spec: FormatSpec = serde_json::from_slice(&bytes)?;

    if spec.version != FORMAT_VERSION {
        let spec_version = &spec.version;
        anyhow::bail!("format version mismatch: spec={spec_version} rust={FORMAT_VERSION}");
    }
    if spec.template != TEMPLATE {
        let spec_template = &spec.template;
        anyhow::bail!("format template mismatch:\n  spec: {spec_template:?}\n  rust: {TEMPLATE:?}");
    }
    let spec_fields: Vec<&str> = spec.fields.iter().map(String::as_str).collect();
    if spec_fields.as_slice() != FIELDS {
        anyhow::bail!("format fields mismatch: spec={spec_fields:?} rust={FIELDS:?}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_per_template() {
        let c = CourseInput {
            subject_code: "MATH".into(),
            catalog_number: "101".into(),
            course_title: "Calculus I".into(),
        };
        assert_eq!(format_input(&c), "MATH 101 --- Calculus I");
    }
}
