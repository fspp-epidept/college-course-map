# Samples

Small example input files for demos, testing, and onboarding. Tracked in git, unlike the gitignored `/data/` dev datasets.

| File | Contents |
| --- | --- |
| `sample_courses.xlsx` | 49,537 course rows (`instnm, sub_pref, course, course_title, academic_year`), provided 2026-07-30 |
| `sample_courses.csv` | CSV conversion of the xlsx's single sheet; the app imports CSV, and these headers auto-map (`sub_pref` → subject, `course` → catalog number, `course_title` → title) |
