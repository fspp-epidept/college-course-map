-- Input profile computed by the import worker (profile.rs): field shapes,
-- checks, findings, and sample model inputs. NULL for datasets imported
-- before the profile existed; the UI says so instead of recomputing.
ALTER TABLE datasets ADD COLUMN input_profile JSON;
