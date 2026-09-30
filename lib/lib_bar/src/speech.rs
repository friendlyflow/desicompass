//! Super+T: say the time, through speech-dispatcher.
//!
//! `spd-say` rather than the screen reader: the bar never has the keyboard,
//! so a screen reader would not read it, and the time should be heard whether
//! or not one is running. speech-dispatcher is what Orca speaks through too,
//! so the two share one voice and one queue. The bar never starts or stops
//! Orca.

use std::process::Command;

use desicompass_bar_protocol::clock::{LocalTime, speech_language, spoken_text};

/// The `spd-say` command line that says `text` in `language`.
pub fn command(text: &str, language: &str) -> Vec<String> {
    vec![
        "spd-say".to_owned(),
        "--language".to_owned(),
        speech_language(language).to_owned(),
        // Above ordinary text, so it is not queued behind a long reading.
        "--priority".to_owned(),
        "message".to_owned(),
        "--".to_owned(),
        text.to_owned(),
    ]
}

/// Say the time now, in `language`, without waiting for it to be said.
pub fn say_time(language: &str) {
    let text = spoken_text(&LocalTime::now(), language);
    let argv = command(&text, language);
    tracing::info!("saying {text:?}");
    let spawned = std::thread::Builder::new()
        .name("bar-speech".into())
        .spawn(
            move || match Command::new(&argv[0]).args(&argv[1..]).status() {
                Ok(s) if s.success() => {}
                Ok(s) => tracing::warn!("spd-say failed ({s})"),
                Err(e) => tracing::warn!("cannot say the time, no spd-say: {e}"),
            },
        );
    if let Err(e) = spawned {
        tracing::warn!("cannot say the time: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_speaks_in_the_session_language_above_ordinary_text() {
        assert_eq!(
            command("Het is 14:05", "nl-BE"),
            [
                "spd-say",
                "--language",
                "nl",
                "--priority",
                "message",
                "--",
                "Het is 14:05"
            ]
        );
    }

    #[test]
    fn text_starting_with_a_dash_is_not_an_option() {
        let argv = command("-x", "en-US");
        assert_eq!(&argv[argv.len() - 2..], ["--", "-x"]);
    }
}
