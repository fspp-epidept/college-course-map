//! `cargo run --example runtime_install [-- <pack-id>...]` (wrapped as
//! `task runtimes:fetch`).
//!
//! Downloads + verifies + extracts the named runtime packs (default: `cpu`)
//! for the current platform. The `cpu` pack is a build input — it lands in
//! `src-tauri/runtimes/cpu/`, where `tauri.conf.json` bundles it as a
//! resource and the check examples load it from. GPU packs are user data —
//! they land in the same data-dir location the in-app download uses, so a
//! dev fetch and a production download are interchangeable.
//! Already-installed packs (matching `.sha256` marker) are skipped, so
//! re-running is cheap and a manifest re-pin naturally refreshes.

use std::path::PathBuf;

use course_classifier_lib::runtime;

fn main() -> anyhow::Result<()> {
    let requested: Vec<String> = {
        let args: Vec<String> = std::env::args().skip(1).collect();
        if args.is_empty() {
            vec!["cpu".to_owned()]
        } else {
            args
        }
    };

    let manifest = runtime::load_manifest().map_err(|e| anyhow::anyhow!(e))?;
    let packs = runtime::packs_for_target(&manifest);
    if packs.is_empty() {
        anyhow::bail!("no runtime packs pinned for this platform");
    }
    let dest_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("runtimes");
    let client = reqwest::blocking::Client::builder()
        .user_agent("college-course-map")
        .timeout(None) // GPU packs are ~200-280 MB; no global deadline
        .build()?;

    for id in &requested {
        let Some(pack) = packs.iter().find(|p| &p.id == id) else {
            let available: Vec<&str> = packs.iter().map(|p| p.id.as_str()).collect();
            anyhow::bail!("unknown pack '{id}' for this platform (available: {available:?})");
        };
        let dest = if pack.id == "cpu" {
            dest_root.join(&pack.id)
        } else {
            runtime::pack_dir(&manifest, pack).map_err(|e| anyhow::anyhow!(e))?
        };
        if runtime::installed(&dest, pack) {
            println!("{id}: skipped (already installed at {})", dest.display());
            continue;
        }
        let total = pack.total_size();
        println!(
            "{id}: downloading {} archive(s), {total} bytes total",
            pack.archives.len()
        );
        let mut last_pct: u64 = 0;
        runtime::install_pack(&client, pack, &dest, &mut |received, bps| {
            let pct = received * 100 / total.max(1);
            if pct >= last_pct + 10 {
                println!("  {pct}% ({:.1} MB/s)", bps / 1_000_000.0);
                last_pct = pct;
            }
        })
        .map_err(|e| anyhow::anyhow!(e))?;
        println!("{id}: installed at {}", dest.display());
    }

    println!("Done.");
    Ok(())
}
