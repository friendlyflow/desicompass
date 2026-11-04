//! Drift guards for what the .deb and the .rpm ship, which no compiler checks.
//! The lists are `[package.metadata.deb]` and `[package.metadata.generate-rpm]`
//! in Cargo.toml.

use std::collections::BTreeSet;
use std::path::Path;

fn read(path: &str) -> String {
    let full = Path::new(env!("CARGO_MANIFEST_DIR")).join(path);
    std::fs::read_to_string(&full).unwrap_or_else(|e| panic!("reading {}: {e}", full.display()))
}

/// The lines of the table `[name]` in a TOML file.
fn table<'a>(toml: &'a str, name: &str) -> Vec<&'a str> {
    let header = format!("[{name}]");
    toml.lines()
        .skip_while(|l| *l != header)
        .skip(1)
        .take_while(|l| !l.starts_with('['))
        .collect()
}

/// The double-quoted strings on one line.
fn strings(line: &str) -> Vec<&str> {
    line.split('"').skip(1).step_by(2).collect()
}

/// `(source, destination)` of each asset in the .deb's list. Destinations get a
/// leading slash, the way the .rpm's list spells them.
fn deb_assets(toml: &str) -> BTreeSet<(String, String)> {
    table(toml, "package.metadata.deb")
        .iter()
        .filter(|l| l.trim_start().starts_with("[\""))
        .map(|l| {
            let s = strings(l);
            (s[0].to_owned(), format!("/{}", s[1]))
        })
        .collect()
}

fn rpm_assets(toml: &str) -> BTreeSet<(String, String)> {
    table(toml, "package.metadata.generate-rpm")
        .iter()
        .filter(|l| l.trim_start().starts_with("{ source"))
        .map(|l| {
            let s = strings(l);
            (s[0].to_owned(), s[1].to_owned())
        })
        .collect()
}

#[test]
fn deb_and_rpm_ship_the_same_files() {
    let toml = read("Cargo.toml");
    let deb = deb_assets(&toml);
    let rpm = rpm_assets(&toml);
    assert!(!deb.is_empty() && !rpm.is_empty(), "no assets found");
    assert_eq!(deb, rpm);
}

#[test]
fn packages_ship_all_three_programs_and_their_licenses() {
    let toml = read("Cargo.toml");
    let assets = rpm_assets(&toml);
    for bin in ["desicompass", "desicompass-superkey", "desicompass-bar"] {
        let asset = (format!("target/release/{bin}"), format!("/usr/bin/{bin}"));
        assert!(assets.contains(&asset), "{bin} is not in the packages");
    }
    let sources: Vec<&str> = assets.iter().map(|(s, _)| s.as_str()).collect();
    assert!(sources.contains(&"THIRD-PARTY-LICENSES.html"));
    assert!(
        sources.contains(&"lib/lib_superkey/fonts/LICENSE-*.txt"),
        "the fonts are inside the binaries, so their licenses have to ship"
    );
}

#[test]
fn every_packaged_file_exists_in_the_checkout() {
    let toml = read("Cargo.toml");
    for (source, _) in rpm_assets(&toml) {
        if source.starts_with("target/") || source.contains('*') {
            continue;
        }
        let full = Path::new(env!("CARGO_MANIFEST_DIR")).join(&source);
        assert!(full.is_file(), "{source} is packaged but does not exist");
    }
}

/// A floor below the release the superkey's Store was built against lets it
/// install plugins for an app that cannot run them. The pin is a `rev` between
/// releases, which says nothing about a version, so this only holds at a tag.
#[test]
fn the_sicompass_floor_follows_the_pin() {
    let manifest = read("lib/lib_superkey/Cargo.toml");
    let line = manifest
        .lines()
        .find(|l| l.starts_with("sicompass-store"))
        .expect("the superkey pins sicompass-store");
    let Some(tag) = line
        .split("tag = \"v")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
    else {
        return;
    };
    let toml = read("Cargo.toml");
    let deb = format!("sicompass (>= {tag})");
    let rpm = format!("sicompass = \">= {tag}\"");
    assert!(toml.contains(&deb), "the .deb should depend on {deb}");
    assert!(toml.contains(&rpm), "the .rpm should require {rpm}");
    let flake = format!("github:friendlyflow/sicompass/v{tag}");
    assert!(
        read("flake.nix").contains(&flake),
        "the flake's sicompass input should be {flake}"
    );
}

#[test]
fn the_session_entry_starts_the_session_script() {
    let entry = read("packaging/desicompass.desktop");
    assert!(entry.contains("Exec=/usr/bin/desicompass-session\n"));
    // loginsicompass reads this name for XDG_CURRENT_DESKTOP.
    assert!(entry.contains("DesktopNames=Desicompass\n"));
    assert!(
        !entry.contains('\'') && !entry.contains("\"\""),
        "the Exec line must hold no quoting, the spec gives it no agreed meaning"
    );
}

/// Each of these cost someone a silent or a dead session on NixOS, where the
/// module's comments tell the stories.
#[test]
fn the_session_script_keeps_what_is_load_bearing() {
    let script = read("packaging/desicompass-session");
    for word in [
        "systemd-cat --identifier=desicompass",
        "dbus-run-session",
        "--backend tty",
        "sicompass --session",
    ] {
        assert!(script.contains(word), "the session script lost `{word}`");
    }
}

#[test]
fn the_shared_accessibility_folder_belongs_to_its_group() {
    let tmpfiles = read("packaging/desicompass.tmpfiles");
    assert!(tmpfiles.contains("d /var/lib/sicompass 2775 root sicompass-a11y"));
    assert!(read("packaging/desicompass.sysusers").contains("g sicompass-a11y"));
    assert!(read("packaging/scripts/postinst").contains("usermod -aG sicompass-a11y"));
}

/// Dlopened, so no scan finds them and the ELF headers do not name them. Without
/// libwayland-server the compositor exits with `NoWaylandLib`, and no desktop
/// lacks it, so only a clean install (scripts/check-install.sh) ever shows it.
#[test]
fn dlopened_libraries_are_declared_in_both_packages() {
    let toml = read("Cargo.toml");
    let deb = table(&toml, "package.metadata.deb").join("\n");
    let rpm = table(&toml, "package.metadata.generate-rpm").join("\n");
    for (package, soname) in [
        ("libegl1", "libEGL.so.1"),
        ("libgles2", "libGLESv2.so.2"),
        ("libwayland-server0", "libwayland-server.so.0"),
        ("libwayland-client0", "libwayland-client.so.0"),
        ("libwayland-egl1", "libwayland-egl.so.1"),
        ("libwayland-cursor0", "libwayland-cursor.so.0"),
        ("libvulkan1", "libvulkan.so.1"),
    ] {
        assert!(deb.contains(package), "the .deb should depend on {package}");
        assert!(
            rpm.contains(&format!("\"{soname}()(64bit)\"")),
            "the .rpm should require {soname}"
        );
    }
}

/// The tool's own scan lists what the libraries link too, and Ubuntu's are not
/// Fedora's: that made the .rpm impossible to install.
#[test]
fn the_rpm_requires_only_what_the_binaries_link_directly() {
    let script = read("scripts/release-package.sh");
    assert!(script.contains("--auto-req packaging/rpm-find-requires"));
    assert!(read("packaging/rpm-find-requires").contains("NEEDED"));
}
