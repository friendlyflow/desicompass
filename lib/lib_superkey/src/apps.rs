//! The installed programs: every visible `Type=Application` desktop entry.
//!
//! A small parser rather than a desktop-entry crate, for the reason
//! loginsicompass's `sessions.rs` gives (from which [`split_exec`] and the
//! name matching are copied): one group and a handful of keys matter. This
//! adds what an application menu needs and a session list does not:
//! `TryExec`, `OnlyShowIn`/`NotShowIn`, `Terminal`, `Path`, desktop file ids,
//! and the precedence rule that a user's own entry (even a `Hidden=true` one)
//! replaces the system's.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// One program the superkey offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct App {
    /// The desktop file id, e.g. `org.gnome.Nautilus` or `kde-okular`: the
    /// path below `applications/` with `/` turned into `-`, minus `.desktop`.
    /// What the superkey's button names, since `name` depends on the language.
    pub id: String,
    /// `Name=`, locale-matched.
    pub name: String,
    /// `Exec=`, split into an argv with the field codes dropped.
    pub exec: Vec<String>,
    /// `Path=`, the directory to start it in.
    pub path: Option<String>,
}

/// Parse one desktop entry. `None` when it is not something to offer: not an
/// application, `Hidden`, `NoDisplay`, meant for another desktop, a terminal
/// program (nothing here can give it a terminal yet), a `TryExec` that is not
/// installed, or no usable `Exec`.
///
/// `locale` is the language to prefer for `Name[..]`, in `ll_CC` form, and
/// `desktops` the names in `XDG_CURRENT_DESKTOP`.
pub fn parse_desktop_entry(
    contents: &str,
    id: &str,
    locale: Option<&str>,
    desktops: &[String],
    installed: impl Fn(&str) -> bool,
) -> Option<App> {
    let mut in_entry = false;
    let mut name: Option<String> = None;
    let mut localized: Vec<(String, String)> = Vec::new();
    let mut exec: Option<String> = None;
    let mut try_exec: Option<String> = None;
    let mut path: Option<String> = None;
    let mut ty: Option<String> = None;
    let mut only_show_in: Option<Vec<String>> = None;
    let mut not_show_in: Vec<String> = Vec::new();
    let mut hidden = false;
    let mut no_display = false;
    let mut terminal = false;

    for raw in contents.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            // Only [Desktop Entry]: actions and vendor groups are ignored.
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_entry {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        let is_true = || value.eq_ignore_ascii_case("true");
        match key {
            "Type" => ty = Some(value.to_owned()),
            "Name" => name = Some(value.to_owned()),
            "Exec" => exec = Some(value.to_owned()),
            "TryExec" => try_exec = Some(value.to_owned()),
            "Path" if !value.is_empty() => path = Some(value.to_owned()),
            "Hidden" => hidden = is_true(),
            "NoDisplay" => no_display = is_true(),
            "Terminal" => terminal = is_true(),
            "OnlyShowIn" => only_show_in = Some(split_list(value)),
            "NotShowIn" => not_show_in = split_list(value),
            k if k.starts_with("Name[") && k.ends_with(']') => {
                localized.push((k["Name[".len()..k.len() - 1].to_owned(), value.to_owned()));
            }
            _ => {}
        }
    }

    if ty.as_deref() != Some("Application") || hidden || no_display || terminal {
        return None;
    }
    if let Some(only) = &only_show_in
        && !only.iter().any(|d| desktops.contains(d))
    {
        return None;
    }
    if not_show_in.iter().any(|d| desktops.contains(d)) {
        return None;
    }
    if let Some(t) = &try_exec
        && !installed(t)
    {
        return None;
    }
    let argv = split_exec(&exec?);
    if argv.is_empty() {
        return None;
    }
    Some(App {
        id: id.to_owned(),
        name: pick_name(name, &localized, locale)?,
        exec: argv,
        path,
    })
}

/// A `;`-separated desktop-entry list.
fn split_list(value: &str) -> Vec<String> {
    value
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Choose `Name`, preferring an exact locale match then its language part.
/// Copied from loginsicompass's `sessions.rs`.
fn pick_name(
    base: Option<String>,
    localized: &[(String, String)],
    locale: Option<&str>,
) -> Option<String> {
    if let Some(loc) = locale {
        let loc = loc.split('.').next().unwrap_or(loc);
        if let Some((_, v)) = localized.iter().find(|(t, _)| t == loc) {
            return Some(v.clone());
        }
        let lang = loc.split('_').next().unwrap_or(loc);
        if let Some((_, v)) = localized.iter().find(|(t, _)| t == lang) {
            return Some(v.clone());
        }
    }
    base
}

/// Split an `Exec=` value into an argv, per the Desktop Entry specification.
/// Copied from loginsicompass's `sessions.rs`: double-quote grouping,
/// backslash escapes inside quotes, `%%` as a literal `%`, and the field codes
/// dropped (the superkey opens programs, never files).
pub fn split_exec(exec: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut has_cur = false;
    let mut in_quotes = false;
    let mut chars = exec.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '"' => {
                in_quotes = !in_quotes;
                has_cur = true;
            }
            '\\' if in_quotes => {
                if let Some(next) = chars.next() {
                    cur.push(next);
                    has_cur = true;
                }
            }
            // `%%` is a literal percent; any other field code expands to
            // nothing here.
            '%' => {
                if let Some('%') = chars.next() {
                    cur.push('%');
                    has_cur = true;
                }
            }
            c if c.is_whitespace() && !in_quotes => {
                if has_cur {
                    out.push(std::mem::take(&mut cur));
                    has_cur = false;
                }
            }
            c => {
                cur.push(c);
                has_cur = true;
            }
        }
    }
    if has_cur {
        out.push(cur);
    }
    while out.last().is_some_and(|s| s.is_empty()) {
        out.pop();
    }
    out
}

/// Whether a `TryExec` program is installed: an absolute path that is an
/// executable file, or a name found on `PATH`.
pub fn is_installed(program: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let executable = |p: &Path| {
        std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    };
    if program.contains('/') {
        return executable(Path::new(program));
    }
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|d| executable(&d.join(program))))
        .unwrap_or(false)
}

/// The `applications` directories to search, highest precedence first.
///
/// `$XDG_DATA_HOME` then `$XDG_DATA_DIRS` (or the spec's defaults), then the
/// NixOS profile directories, which a session's `XDG_DATA_DIRS` usually has
/// but not always: the system profile, the per-user profile and `~/.nix-profile`.
pub fn search_dirs(
    data_home: Option<&str>,
    data_dirs: Option<&str>,
    home: Option<&Path>,
    user: Option<&str>,
    extra: &[PathBuf],
) -> Vec<PathBuf> {
    let mut bases: Vec<PathBuf> = Vec::new();
    match data_home.filter(|s| !s.is_empty()) {
        Some(d) => bases.push(PathBuf::from(d)),
        None => {
            if let Some(h) = home {
                bases.push(h.join(".local/share"));
            }
        }
    }
    match data_dirs.filter(|s| !s.is_empty()) {
        Some(s) => bases.extend(s.split(':').filter(|p| !p.is_empty()).map(PathBuf::from)),
        None => {
            bases.push(PathBuf::from("/usr/local/share"));
            bases.push(PathBuf::from("/usr/share"));
        }
    }
    let mut nix = Vec::new();
    if let Some(h) = home {
        nix.push(h.join(".nix-profile/share"));
    }
    if let Some(u) = user {
        nix.push(PathBuf::from(format!("/etc/profiles/per-user/{u}/share")));
    }
    nix.push(PathBuf::from("/run/current-system/sw/share"));
    for n in nix {
        if !bases.contains(&n) {
            bases.push(n);
        }
    }

    let mut out: Vec<PathBuf> = extra.to_vec();
    for b in bases {
        let d = b.join("applications");
        if !out.contains(&d) {
            out.push(d);
        }
    }
    out
}

/// Every `.desktop` file below `dir`, with its desktop file id.
fn entries_in(dir: &Path) -> Vec<(String, PathBuf)> {
    fn walk(root: &Path, dir: &Path, depth: usize, out: &mut Vec<(String, PathBuf)>) {
        // Symlink loops exist in the wild; nobody nests menus this deep.
        if depth > 8 {
            return;
        }
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        let mut items: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
        items.sort();
        for p in items {
            if p.is_dir() {
                walk(root, &p, depth + 1, out);
            } else if p.extension().is_some_and(|e| e == "desktop")
                && let Ok(rel) = p.strip_prefix(root)
            {
                let rel = rel.to_string_lossy();
                let id = rel.trim_end_matches(".desktop").replace('/', "-");
                out.push((id, p));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, 0, &mut out);
    out
}

/// Every offered program in `dirs`, sorted by name.
///
/// The first directory to have an id decides it, whatever it says: a user's
/// `Hidden=true` copy in `~/.local/share/applications` removes the system's
/// entry, as the specification requires.
pub fn scan(dirs: &[PathBuf], locale: Option<&str>, desktops: &[String]) -> Vec<App> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut apps: Vec<App> = Vec::new();
    for dir in dirs {
        for (id, path) in entries_in(dir) {
            if !seen.insert(id.clone()) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            if let Some(app) = parse_desktop_entry(&text, &id, locale, desktops, is_installed) {
                apps.push(app);
            }
        }
    }
    apps.sort_by_cached_key(|a| (a.name.to_lowercase(), a.id.clone()));
    apps
}

/// The programs, rescanned only when a directory changed.
///
/// Scanning is cheap but not free, and it runs every time the superkey opens.
/// A directory's stamp is its resolved path and modification time: on NixOS
/// the profile's `applications` is a symlink into the store, where every
/// mtime is 1, and a rebuild changes where it points instead.
#[derive(Debug)]
pub struct Catalogue {
    dirs: Vec<PathBuf>,
    locale: Option<String>,
    desktops: Vec<String>,
    stamps: Vec<Option<(PathBuf, Option<SystemTime>)>>,
    apps: Vec<App>,
}

impl Catalogue {
    pub fn new(dirs: Vec<PathBuf>, locale: Option<String>, desktops: Vec<String>) -> Self {
        let mut c = Self {
            dirs,
            locale,
            desktops,
            stamps: Vec::new(),
            apps: Vec::new(),
        };
        c.rescan();
        c
    }

    /// The desktop names in `XDG_CURRENT_DESKTOP`.
    pub fn current_desktops() -> Vec<String> {
        std::env::var("XDG_CURRENT_DESKTOP")
            .map(|v| {
                v.split(':')
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// A sicompass language (`nl-BE`) as a desktop-entry locale (`nl_BE`).
    pub fn locale_for(language: &str) -> String {
        language.replace('-', "_")
    }

    pub fn apps(&self) -> &[App] {
        &self.apps
    }

    pub fn find(&self, id: &str) -> Option<&App> {
        self.apps.iter().find(|a| a.id == id)
    }

    /// Rescan if any directory changed. Returns whether the list may differ.
    pub fn refresh(&mut self) -> bool {
        if self.current_stamps() == self.stamps {
            return false;
        }
        self.rescan();
        true
    }

    /// Names are per language: rescan for a new one.
    pub fn set_locale(&mut self, locale: Option<String>) {
        if locale != self.locale {
            self.locale = locale;
            self.rescan();
        }
    }

    fn current_stamps(&self) -> Vec<Option<(PathBuf, Option<SystemTime>)>> {
        self.dirs
            .iter()
            .map(|d| {
                let real = std::fs::canonicalize(d).ok()?;
                let modified = std::fs::metadata(&real).ok()?.modified().ok();
                Some((real, modified))
            })
            .collect()
    }

    fn rescan(&mut self) {
        self.stamps = self.current_stamps();
        self.apps = scan(&self.dirs, self.locale.as_deref(), &self.desktops);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Option<App> {
        parse_desktop_entry(text, "x", Some("nl_BE"), &["Desicompass".to_owned()], |p| {
            p == "/usr/bin/present" || p == "present"
        })
    }

    const FOOT: &str =
        "[Desktop Entry]\nType=Application\nName=Foot\nName[nl]=Voet\nExec=foot %U\n";

    #[test]
    fn an_application_is_read_with_its_localised_name() {
        let a = parse(FOOT).unwrap();
        assert_eq!(a.name, "Voet", "Name[nl] matches nl_BE by language");
        assert_eq!(a.exec, ["foot"], "the field code is dropped");
    }

    #[test]
    fn what_is_not_offered() {
        for extra in [
            "Hidden=true",
            "NoDisplay=true",
            "Terminal=true",
            "OnlyShowIn=GNOME;KDE;",
            "NotShowIn=Desicompass;",
            "TryExec=/usr/bin/absent",
        ] {
            assert_eq!(parse(&format!("{FOOT}{extra}\n")), None, "{extra}");
        }
        assert_eq!(parse(&FOOT.replace("Type=Application", "Type=Link")), None);
        assert_eq!(parse(&FOOT.replace("Type=Application\n", "")), None);
        assert_eq!(parse(&FOOT.replace("Exec=foot %U", "Exec=%U")), None);
    }

    #[test]
    fn what_is_offered_after_all() {
        assert!(parse(&format!("{FOOT}OnlyShowIn=Desicompass;\n")).is_some());
        assert!(parse(&format!("{FOOT}NotShowIn=GNOME;\n")).is_some());
        assert!(parse(&format!("{FOOT}TryExec=/usr/bin/present\n")).is_some());
        assert!(parse(&format!("{FOOT}TryExec=present\n")).is_some());
    }

    #[test]
    fn only_the_desktop_entry_group_counts() {
        let text = format!(
            "{FOOT}[Desktop Action new]\nName=New window\nExec=foot --new\nNoDisplay=true\n"
        );
        let a = parse(&text).unwrap();
        assert_eq!(a.exec, ["foot"]);
        assert_eq!(a.name, "Voet");
    }

    #[test]
    fn exec_is_split_like_the_specification_says() {
        assert_eq!(
            split_exec(r#""/opt/My App/bin/app" --title "a \"b\"" 100%% %f"#),
            ["/opt/My App/bin/app", "--title", "a \"b\"", "100%"]
        );
        assert_eq!(split_exec("env A=1 prog %F"), ["env", "A=1", "prog"]);
    }

    #[test]
    fn the_path_key_is_kept() {
        let a = parse(&format!("{FOOT}Path=/srv/work\n")).unwrap();
        assert_eq!(a.path.as_deref(), Some("/srv/work"));
    }

    fn write(dir: &Path, rel: &str, text: &str) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    #[test]
    fn a_user_entry_overrides_and_can_hide_a_system_one() {
        let user = tempfile::tempdir().unwrap();
        let system = tempfile::tempdir().unwrap();
        write(system.path(), "foot.desktop", FOOT);
        write(
            system.path(),
            "gimp.desktop",
            "[Desktop Entry]\nType=Application\nName=GIMP\nExec=gimp\n",
        );
        write(
            user.path(),
            "gimp.desktop",
            "[Desktop Entry]\nType=Application\nName=GIMP\nExec=gimp\nHidden=true\n",
        );
        write(
            user.path(),
            "foot.desktop",
            &FOOT.replace("Exec=foot %U", "Exec=foot --server"),
        );
        let apps = scan(&[user.path().into(), system.path().into()], None, &[]);
        assert_eq!(apps.len(), 1, "{apps:?}");
        assert_eq!(apps[0].exec, ["foot", "--server"]);
    }

    #[test]
    fn ids_come_from_the_path_and_the_list_is_sorted_by_name() {
        let d = tempfile::tempdir().unwrap();
        write(
            d.path(),
            "kde/okular.desktop",
            "[Desktop Entry]\nType=Application\nName=okular\nExec=okular\n",
        );
        write(
            d.path(),
            "org.b.desktop",
            "[Desktop Entry]\nType=Application\nName=Bravo\nExec=b\n",
        );
        write(d.path(), "not-an-entry.txt", "hello");
        let apps = scan(&[d.path().into()], None, &[]);
        let ids: Vec<_> = apps.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(
            ids,
            ["org.b", "kde-okular"],
            "Bravo before okular, case-insensitively"
        );
    }

    #[test]
    fn the_catalogue_rescans_only_when_a_directory_changes() {
        let d = tempfile::tempdir().unwrap();
        write(d.path(), "foot.desktop", FOOT);
        let mut c = Catalogue::new(vec![d.path().into()], None, vec![]);
        assert_eq!(c.apps().len(), 1);
        assert!(!c.refresh());
        // A new entry changes the directory's mtime (after a moment, for
        // filesystems with coarse timestamps).
        std::thread::sleep(std::time::Duration::from_millis(20));
        write(
            d.path(),
            "b.desktop",
            "[Desktop Entry]\nType=Application\nName=B\nExec=b\n",
        );
        let now = std::time::SystemTime::now();
        let f = std::fs::File::open(d.path()).unwrap();
        f.set_modified(now + std::time::Duration::from_secs(1)).ok();
        assert!(c.refresh());
        assert_eq!(c.apps().len(), 2);
        assert!(c.find("b").is_some());
    }

    #[test]
    fn search_dirs_put_the_user_first_and_add_the_nix_profiles() {
        let dirs = search_dirs(
            None,
            Some("/a:/b"),
            Some(Path::new("/home/u")),
            Some("u"),
            &[PathBuf::from("/extra")],
        );
        assert_eq!(
            dirs,
            [
                "/extra",
                "/home/u/.local/share/applications",
                "/a/applications",
                "/b/applications",
                "/home/u/.nix-profile/share/applications",
                "/etc/profiles/per-user/u/share/applications",
                "/run/current-system/sw/share/applications",
            ]
            .map(PathBuf::from)
        );
    }

    #[test]
    fn a_language_is_turned_into_a_desktop_locale() {
        assert_eq!(Catalogue::locale_for("nl-BE"), "nl_BE");
    }
}
