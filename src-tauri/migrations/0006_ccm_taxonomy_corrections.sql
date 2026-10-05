-- CCM taxonomy corrections.
--
-- The 6-digit CSV seeded by 0003 came from a flawed text extraction of the
-- NCES report. It was corrected against the official PDF (NCES 2012-162rev,
-- "2010 College Course Map", bundled with the app): Mac-Roman mojibake
-- restored to the typographic characters the PDF uses (46 rows), titles cut
-- at a PDF line wrap restored and their spill removed from the neighbouring
-- field (13.9998, 35.9994, 35.0199, 39.9998), a neighbouring code's
-- description stripped from 40.0201, 40.1001 and 46.0505, page-footer text
-- stripped from 44.0702 and 51.2212, and missing spaces / final periods
-- restored. Codes were already 1:1 with the PDF at both levels; 2-digit rows
-- were unaffected (their title_short is app-authored, not PDF text).
--
-- The DELETE empties the table; the post-migration data hook (db.rs)
-- seeds it from the corrected CSVs in the same transaction. Nothing
-- references ccm_taxonomy by foreign key — readers LEFT JOIN by code.

DELETE FROM ccm_taxonomy;
