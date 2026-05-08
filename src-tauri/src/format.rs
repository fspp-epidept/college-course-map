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

pub fn format_input(course: &CourseInput) -> String {
    format!(
        "{} {} --- {}",
        course.subject_code, course.catalog_number, course.course_title
    )
}

#[derive(Debug, Deserialize)]
struct FormatSpec {
    version: String,
    template: String,
    fields: Vec<String>,
}

pub fn assert_matches_spec(spec_path: &Path) -> anyhow::Result<()> {
    let bytes = std::fs::read(spec_path)
        .map_err(|e| anyhow::anyhow!("read {}: {}", spec_path.display(), e))?;
    let spec: FormatSpec = serde_json::from_slice(&bytes)?;

    if spec.version != FORMAT_VERSION {
        anyhow::bail!(
            "format version mismatch: spec={} rust={}",
            spec.version,
            FORMAT_VERSION
        );
    }
    if spec.template != TEMPLATE {
        anyhow::bail!(
            "format template mismatch:\n  spec: {:?}\n  rust: {:?}",
            spec.template,
            TEMPLATE
        );
    }
    let spec_fields: Vec<&str> = spec.fields.iter().map(String::as_str).collect();
    if spec_fields.as_slice() != FIELDS {
        anyhow::bail!(
            "format fields mismatch: spec={:?} rust={:?}",
            spec_fields,
            FIELDS
        );
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
