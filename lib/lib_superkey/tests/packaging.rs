//! Drift guards for what the package ships but no compiler checks.

/// The renderer (`sicompass-ui`) compiles the fonts into this binary, so the
/// package has to ship their license texts. `fonts/` keeps copies for
/// `flake.nix` to install, and those have to be the texts of the fonts actually
/// linked.
#[test]
fn shipped_font_licenses_match_the_linked_renderer() {
    assert!(
        !sicompass_ui::fonts::LICENSES.is_empty(),
        "sicompass-ui exposes no font licenses to check"
    );
    for (name, text) in sicompass_ui::fonts::LICENSES {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fonts")
            .join(name);
        let shipped = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
        assert_eq!(
            shipped, *text,
            "fonts/{name} differs from the one in the sicompass-ui version this \
             build links. Copy it over from that repo's fonts/."
        );
    }
}
