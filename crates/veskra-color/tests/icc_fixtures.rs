//! The committed generated ICC files must equal what the writer produces today.
//!
//! Regenerate with `VESKRA_REGEN_COLOR_FIXTURES=1 cargo test -p veskra-color --test icc_fixtures`
//! (then re-run `scripts/color/gen_oracle_fixtures.py`).

// Test-only code: fixture/IO failures should abort the test loudly.
#![allow(clippy::unwrap_used, clippy::needless_range_loop)]

use std::path::PathBuf;
use veskra_color::IccProfile;

fn icc_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/color/icc")
}

#[test]
fn generated_icc_fixtures_are_current() {
    let regen = std::env::var("VESKRA_REGEN_COLOR_FIXTURES").is_ok_and(|v| v == "1");
    for (name, profile) in
        [("veskra-srgb-v4.icc", IccProfile::srgb()), ("veskra-display-p3-v4.icc", IccProfile::display_p3())]
    {
        let path = icc_dir().join(name);
        if regen {
            std::fs::create_dir_all(icc_dir()).unwrap();
            std::fs::write(&path, profile.bytes()).unwrap();
        }
        let on_disk = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert_eq!(on_disk, profile.bytes(), "{name} is stale; regenerate (see file header)");
        let reparsed = IccProfile::parse(&on_disk).unwrap();
        assert_eq!(reparsed.id(), profile.id());
    }
}
