//! The clock, as the bar shows it, the superkey lists it and Super+D says it.
//!
//! Hand-rolled like the login screen's (`loginsicompass/src/provider.rs`):
//! three formats in four languages are the whole need, and `chrono` would be
//! a dependency for what `localtime_r` and a date algorithm already do. The
//! names live here rather than in each program's Fluent files so the bar and
//! the superkey cannot disagree about them.

/// A moment in local time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalTime {
    pub year: i64,
    /// 1 to 12.
    pub month: u32,
    /// 1 to 31.
    pub day: u32,
    /// 0 is Monday, 6 is Sunday.
    pub weekday: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

impl LocalTime {
    /// The local time now.
    pub fn now() -> Self {
        Self::at(unix_now())
    }

    /// The local time at `unix_secs`, in the process's time zone.
    pub fn at(unix_secs: i64) -> Self {
        Self::from_offset(unix_secs, local_utc_offset_secs(unix_secs))
    }

    /// The time at `unix_secs` in a zone `offset_secs` east of UTC.
    pub fn from_offset(unix_secs: i64, offset_secs: i64) -> Self {
        let local = unix_secs + offset_secs;
        let days = local.div_euclid(86_400);
        let secs = local.rem_euclid(86_400);
        let (year, month, day) = civil_from_days(days);
        Self {
            year,
            month: month as u32,
            day: day as u32,
            // 1970-01-01 was a Thursday.
            weekday: (days + 3).rem_euclid(7) as u32,
            hour: (secs / 3600) as u32,
            minute: ((secs % 3600) / 60) as u32,
            second: (secs % 60) as u32,
        }
    }

    /// `14:05`, or `14:05:09` with seconds.
    pub fn time(&self, seconds: bool) -> String {
        if seconds {
            format!("{:02}:{:02}:{:02}", self.hour, self.minute, self.second)
        } else {
            format!("{:02}:{:02}", self.hour, self.minute)
        }
    }
}

/// Seconds since the Unix epoch.
pub fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Seconds east of UTC at `at`, from the `TZ`-aware `localtime_r`. Asked for
/// the moment shown, since the offset changes with daylight saving time.
fn local_utc_offset_secs(at: i64) -> i64 {
    // SAFETY: localtime_r writes into a tm this function owns and reads a
    // time_t it owns; neither pointer escapes.
    unsafe {
        let t = at as libc::time_t;
        let mut out: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&t, &mut out).is_null() {
            return 0;
        }
        out.tm_gmtoff as i64
    }
}

/// Days since the Unix epoch to `(year, month, day)`: Howard Hinnant's
/// `civil_from_days`.
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// The four languages the session speaks, by their first two letters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lang {
    En,
    Nl,
    Fr,
    De,
}

fn lang(code: &str) -> Lang {
    match code.get(..2) {
        Some("nl") => Lang::Nl,
        Some("fr") => Lang::Fr,
        Some("de") => Lang::De,
        _ => Lang::En,
    }
}

/// The language code speech-dispatcher takes for a session language:
/// `nl-BE` is `nl`.
pub fn speech_language(code: &str) -> &'static str {
    match lang(code) {
        Lang::En => "en",
        Lang::Nl => "nl",
        Lang::Fr => "fr",
        Lang::De => "de",
    }
}

const WEEKDAYS: [[&str; 7]; 4] = [
    [
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
        "Sunday",
    ],
    [
        "maandag",
        "dinsdag",
        "woensdag",
        "donderdag",
        "vrijdag",
        "zaterdag",
        "zondag",
    ],
    [
        "lundi", "mardi", "mercredi", "jeudi", "vendredi", "samedi", "dimanche",
    ],
    [
        "Montag",
        "Dienstag",
        "Mittwoch",
        "Donnerstag",
        "Freitag",
        "Samstag",
        "Sonntag",
    ],
];

const WEEKDAYS_SHORT: [[&str; 7]; 4] = [
    ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"],
    ["ma", "di", "wo", "do", "vr", "za", "zo"],
    ["lun.", "mar.", "mer.", "jeu.", "ven.", "sam.", "dim."],
    ["Mo.", "Di.", "Mi.", "Do.", "Fr.", "Sa.", "So."],
];

const MONTHS: [[&str; 12]; 4] = [
    [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ],
    [
        "januari",
        "februari",
        "maart",
        "april",
        "mei",
        "juni",
        "juli",
        "augustus",
        "september",
        "oktober",
        "november",
        "december",
    ],
    [
        "janvier",
        "février",
        "mars",
        "avril",
        "mai",
        "juin",
        "juillet",
        "août",
        "septembre",
        "octobre",
        "novembre",
        "décembre",
    ],
    [
        "Januar",
        "Februar",
        "März",
        "April",
        "Mai",
        "Juni",
        "Juli",
        "August",
        "September",
        "Oktober",
        "November",
        "Dezember",
    ],
];

const MONTHS_SHORT: [[&str; 12]; 4] = [
    [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ],
    [
        "jan", "feb", "mrt", "apr", "mei", "jun", "jul", "aug", "sep", "okt", "nov", "dec",
    ],
    [
        "janv.", "févr.", "mars", "avr.", "mai", "juin", "juil.", "août", "sept.", "oct.", "nov.",
        "déc.",
    ],
    [
        "Jan.", "Feb.", "März", "Apr.", "Mai", "Juni", "Juli", "Aug.", "Sep.", "Okt.", "Nov.",
        "Dez.",
    ],
];

fn names(t: &LocalTime, code: &str, short: bool) -> (Lang, &'static str, &'static str) {
    let l = lang(code);
    let (w, m) = if short {
        (WEEKDAYS_SHORT, MONTHS_SHORT)
    } else {
        (WEEKDAYS, MONTHS)
    };
    let i = l as usize;
    (
        l,
        w[i][t.weekday.min(6) as usize],
        m[i][(t.month.clamp(1, 12) - 1) as usize],
    )
}

/// The bar's clock: `Wed 30 Sep 14:05`, `wo 30 sep 14:05:09`.
pub fn bar_text(t: &LocalTime, language: &str, seconds: bool) -> String {
    let (l, wd, mo) = names(t, language, true);
    let time = t.time(seconds);
    match l {
        Lang::De => format!("{wd}, {}. {mo} {time}", t.day),
        _ => format!("{wd} {} {mo} {time}", t.day),
    }
}

/// The date and time in full, for reading: `Wednesday 30 September 2026, 14:05`.
pub fn long_text(t: &LocalTime, language: &str) -> String {
    format!("{}, {}", long_date(t, language), t.time(false))
}

/// The date in full: `Wednesday 30 September 2026`.
fn long_date(t: &LocalTime, language: &str) -> String {
    let (l, wd, mo) = names(t, language, false);
    match l {
        Lang::De => format!("{wd}, {}. {mo} {}", t.day, t.year),
        _ => format!("{wd} {} {mo} {}", t.day, t.year),
    }
}

/// What Super+D says: the time, then the date,
/// `It is 14:05, Wednesday 30 September 2026`.
pub fn spoken_text(t: &LocalTime, language: &str) -> String {
    let time = match lang(language) {
        Lang::En => format!("It is {}", t.time(false)),
        Lang::Nl => format!("Het is {}", t.time(false)),
        // Written the way French reads a time aloud.
        Lang::Fr => format!("Il est {} h {:02}", t.hour, t.minute),
        Lang::De => format!("Es ist {} Uhr", t.time(false)),
    };
    format!("{time}, {}", long_date(t, language))
}

#[cfg(test)]
mod tests {
    use super::*;

    // 2026-09-30 12:05:09 UTC, a Wednesday.
    const WED: i64 = 1_790_769_909;

    #[test]
    fn the_civil_date_and_weekday_are_right() {
        let t = LocalTime::from_offset(WED, 0);
        assert_eq!((t.year, t.month, t.day), (2026, 9, 30));
        assert_eq!(t.weekday, 2, "Wednesday");
        assert_eq!((t.hour, t.minute, t.second), (12, 5, 9));
        let epoch = LocalTime::from_offset(0, 0);
        assert_eq!(
            (epoch.year, epoch.month, epoch.day, epoch.weekday),
            (1970, 1, 1, 3)
        );
    }

    #[test]
    fn the_offset_moves_the_day_too() {
        // 23:30 UTC on the 30th is the 1st of October in Brussels (UTC+2).
        let late = WED + 11 * 3600 + 25 * 60;
        let t = LocalTime::from_offset(late, 2 * 3600);
        assert_eq!((t.month, t.day, t.weekday, t.hour), (10, 1, 3, 1));
        let t = LocalTime::from_offset(0, -3600);
        assert_eq!((t.year, t.month, t.day, t.hour), (1969, 12, 31, 23));
    }

    #[test]
    fn the_bar_text_in_every_language() {
        let t = LocalTime::from_offset(WED, 2 * 3600);
        assert_eq!(bar_text(&t, "en-US", false), "Wed 30 Sep 14:05");
        assert_eq!(bar_text(&t, "nl-BE", true), "wo 30 sep 14:05:09");
        assert_eq!(bar_text(&t, "fr-BE", false), "mer. 30 sept. 14:05");
        assert_eq!(bar_text(&t, "de-BE", false), "Mi., 30. Sep. 14:05");
        // Anything else is English.
        assert_eq!(bar_text(&t, "", false), "Wed 30 Sep 14:05");
    }

    #[test]
    fn the_long_text_in_every_language() {
        let t = LocalTime::from_offset(WED, 2 * 3600);
        assert_eq!(long_text(&t, "en-US"), "Wednesday 30 September 2026, 14:05");
        assert_eq!(long_text(&t, "nl-BE"), "woensdag 30 september 2026, 14:05");
        assert_eq!(long_text(&t, "fr-BE"), "mercredi 30 septembre 2026, 14:05");
        assert_eq!(
            long_text(&t, "de-BE"),
            "Mittwoch, 30. September 2026, 14:05"
        );
    }

    #[test]
    fn what_super_d_says() {
        let t = LocalTime::from_offset(WED, 2 * 3600);
        assert_eq!(
            spoken_text(&t, "en-US"),
            "It is 14:05, Wednesday 30 September 2026"
        );
        assert_eq!(
            spoken_text(&t, "nl-BE"),
            "Het is 14:05, woensdag 30 september 2026"
        );
        assert_eq!(
            spoken_text(&t, "fr-BE"),
            "Il est 14 h 05, mercredi 30 septembre 2026"
        );
        assert_eq!(
            spoken_text(&t, "de-BE"),
            "Es ist 14:05 Uhr, Mittwoch, 30. September 2026"
        );
        assert_eq!(speech_language("nl-BE"), "nl");
        assert_eq!(speech_language("xx"), "en");
    }

    #[test]
    fn every_language_names_every_day_and_month() {
        for table in [WEEKDAYS, WEEKDAYS_SHORT] {
            for l in table {
                assert!(l.iter().all(|n| !n.is_empty()));
            }
        }
        for table in [MONTHS, MONTHS_SHORT] {
            for l in table {
                assert!(l.iter().all(|n| !n.is_empty()));
            }
        }
    }
}
