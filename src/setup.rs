use crate::{
    Cli, Settings,
    api::{self, Provider},
    keybind,
    mode::{AnswerMode, SelectionMode},
};
use anyhow::{Result, bail};
use std::io::Write;

fn provider_name(provider: Provider) -> &'static str {
    match provider {
        Provider::Anthropic => "Anthropic / Claude",
        Provider::Openai => "OpenAI",
        Provider::Openrouter => "OpenRouter",
        Provider::Gemini => "Google Gemini",
    }
}

fn provider_number(provider: Provider) -> &'static str {
    match provider {
        Provider::Anthropic => "1",
        Provider::Openai => "2",
        Provider::Openrouter => "3",
        Provider::Gemini => "4",
    }
}

fn parse_provider(input: &str) -> Option<(Provider, bool)> {
    match input.to_lowercase().as_str() {
        "1" | "anthropic" | "claude" => Some((Provider::Anthropic, false)),
        "2" | "openai" => Some((Provider::Openai, false)),
        "3" | "openrouter" => Some((Provider::Openrouter, false)),
        "4" | "gemini" | "google" => Some((Provider::Gemini, false)),
        "5" | "custom" | "compatible" => Some((Provider::Openai, true)),
        _ => None,
    }
}

fn configured_keybind(input: &str) -> Result<String> {
    let key = keybind::normalize_keybind(input)?;
    crate::platform::validate_keybind(&key)?;
    Ok(key)
}

pub(crate) fn configured_pause_keybind(input: &str, analyze: &str) -> Result<Option<String>> {
    let input = input.trim();
    if looks_like_secret(input) {
        bail!("Klucz API wpisujesz dopiero na końcu, w ukrytym polu.");
    }
    let pause = match input.to_ascii_lowercase().as_str() {
        "" | "brak" | "off" | "none" => None,
        _ => Some(configured_keybind(input)?),
    };
    crate::platform::validate_keybind_pair(analyze, pause.as_deref())?;
    Ok(pause)
}

fn looks_like_secret(input: &str) -> bool {
    input.starts_with("sk-") || input.starts_with("AIza") || input.starts_with("gsk_")
}

fn secret_warning(out: &mut impl Write) -> Result<()> {
    writeln!(
        out,
        "To wygląda na klucz, nie ustawienie. Klucz wpisujesz dopiero na końcu, w ukrytym polu."
    )?;
    writeln!(
        out,
        "Jeśli wkleiłeś prawdziwy klucz w jawnym polu, unieważnij go i wygeneruj nowy."
    )?;
    Ok(())
}

fn ask_value(
    ask: &mut impl FnMut(&str) -> Result<String>,
    label: &str,
    default: Option<&str>,
) -> Result<String> {
    let label = match default {
        Some(value) => format!("{label} [{value}]: "),
        None => format!("{label}: "),
    };
    let value = ask(&label)?;
    let value = value.trim();
    Ok(if value.is_empty() {
        default.unwrap_or("")
    } else {
        value
    }
    .to_owned())
}

// Image-capable examples verified against provider docs / OpenRouter on 2026-11-24.
// Sources are linked in README.md. This is not an account-specific availability check.
// Keep existing numbers stable; custom IDs remain allowed.
fn example_models(provider: Provider, base: &str) -> &'static [(&'static str, &'static str)] {
    if base.trim_end_matches('/') != provider.default_base() {
        return &[];
    }
    match provider {
        Provider::Anthropic => &[
            ("claude-haiku-5-5", "Claude Haiku 5.5"),
            ("claude-sonnet-5-5", "Claude Sonnet 5.5"),
            ("claude-opus-5-5", "Claude Opus 5.5"),
            ("claude-fable-5-2", "Claude Fable 5.2 (retencja 30 dni)"),
            ("claude-sonnet-4-6", "Claude Sonnet 4.6 (starszy model)"),
        ],
        Provider::Openai => &[
            ("gpt-5.4-mini", "GPT-5.4 mini"),
            ("gpt-5.4", "GPT-5.4"),
            ("gpt-5.2", "GPT-5.2"),
            ("gpt-6-luna", "GPT-6 Luna"),
            ("gpt-6.2-sol", "GPT-6.2 Sol"),
            ("gpt-6.2-astra", "GPT-6.2 Astra"),
        ],
        Provider::Openrouter => &[
            ("anthropic/claude-haiku-5.5", "Claude Haiku 5.5"),
            ("anthropic/claude-sonnet-5.5", "Claude Sonnet 5.5"),
            ("anthropic/claude-opus-5.5", "Claude Opus 5.5"),
            ("openai/gpt-5.4-mini", "GPT-5.4 mini"),
            ("openai/gpt-6-luna", "GPT-6 Luna"),
            ("openai/gpt-6.2-sol", "GPT-6.2 Sol"),
            ("google/gemini-3.8-flash", "Gemini 3.8 Flash"),
            ("google/gemini-4-hydrogen", "Gemini 4 Hydrogen"),
            ("google/gemini-4-carbon", "Gemini 4 Carbon"),
        ],
        Provider::Gemini => &[
            ("gemini-3.5-flash-lite", "Gemini 3.5 Flash-Lite"),
            ("gemini-3.8-flash", "Gemini 3.8 Flash"),
            ("gemini-3.6-flash", "Gemini 3.6 Flash"),
            ("gemini-3.1-flash-lite", "Gemini 3.1 Flash-Lite"),
            ("gemini-4-hydrogen", "Gemini 4 Hydrogen"),
            ("gemini-4-argon", "Gemini 4 Argon"),
            ("gemini-4-neon", "Gemini 4 Neon"),
            ("gemini-4-carbon", "Gemini 4 Carbon"),
        ],
    }
}

fn select_model(input: &str, examples: &[(&str, &str)]) -> Result<String> {
    if !examples.is_empty() && !input.is_empty() && input.bytes().all(|b| b.is_ascii_digit()) {
        let index = input.parse::<usize>().ok().and_then(|n| n.checked_sub(1));
        return index
            .and_then(|i| examples.get(i))
            .map(|(id, _)| id.to_string())
            .ok_or_else(|| {
                anyhow::anyhow!("Wybierz numer z listy albo wpisz pełny identyfikator modelu.")
            });
    }
    Ok(input.to_owned())
}

fn validate_model(model: &str, provider: Provider) -> Result<()> {
    if model.is_empty() {
        bail!("Wpisz identyfikator modelu z panelu dostawcy (nie klucz API).");
    }
    if model.chars().any(char::is_whitespace) || model.len() > 200 {
        bail!("Podaj sam identyfikator modelu, bez spacji, np. nazwę z panelu dostawcy.");
    }
    if model.starts_with("claud-") || model.starts_with("anthropic/claud-") {
        bail!(
            "Literówka: nazwy Claude zaczynają się od claude-, nie claud-. Wybierz przykład z listy lub popraw nazwę."
        );
    }
    if provider == Provider::Gemini && (model.contains('/') || model.contains(':')) {
        bail!("Dla Gemini podaj sam identyfikator, bez models/ ani :generateContent.");
    }
    Ok(())
}

/// Pure, injectable prompt flow: no network, credentials, or writes to settings.
/// Explicit CLI flags remain available for automation and skip only their own question.
pub fn configure(
    cli: &Cli,
    saved: Option<&Settings>,
    ask: &mut impl FnMut(&str) -> Result<String>,
    out: &mut impl Write,
) -> Result<Settings> {
    writeln!(
        out,
        "\nKonfiguracja krok po kroku — maksymalnie 9 opcji (A–I), jedna lub wiele poprawnych odpowiedzi."
    )?;
    writeln!(
        out,
        "Enter akceptuje wartość w nawiasach. Klucz API podasz NA KOŃCU w ukrytym polu.\n"
    )?;

    let default_provider = saved.map(|s| s.provider).unwrap_or(Provider::Anthropic);
    let default_custom = saved
        .is_some_and(|s| s.provider == Provider::Openai && s.base_url != s.provider.default_base());
    let (provider, custom_url, reuse_base) = match cli.provider {
        Some(provider) => (provider, false, true),
        None => {
            writeln!(
                out,
                "1/7 — Z jakiej usługi AI korzystasz? (wybierz nazwę lub numer, NIE klucz)"
            )?;
            writeln!(
                out,
                "  1. Anthropic / Claude\n  2. OpenAI\n  3. OpenRouter\n  4. Google Gemini\n  5. Inny serwer zgodny z OpenAI"
            )?;
            let default_number = if default_custom {
                "5"
            } else {
                provider_number(default_provider)
            };
            let default_name = if default_custom {
                "własny serwer zgodny z OpenAI"
            } else {
                provider_name(default_provider)
            };
            loop {
                out.flush()?;
                let input = ask(&format!("Dostawca [{default_number} — {default_name}]: "))?;
                let input = input.trim();
                if input.is_empty() {
                    break (default_provider, default_custom, true);
                }
                if looks_like_secret(input) {
                    secret_warning(out)?;
                    continue;
                }
                if let Some((provider, custom)) = parse_provider(input) {
                    break (provider, custom, false);
                }
                writeln!(
                    out,
                    "Wybierz 1–5 albo nazwę: anthropic, openai, openrouter, gemini."
                )?;
            }
        }
    };
    let same_provider = saved.filter(|s| s.provider == provider);
    let base_url = match &cli.base_url {
        Some(base) => {
            api::validate_base_url(base)?;
            base.clone()
        }
        None if custom_url => {
            let default = same_provider
                .filter(|s| s.base_url != provider.default_base())
                .map(|s| s.base_url.as_str());
            writeln!(
                out,
                "Adres bazowy własnego API, np. https://host/v1 (bez /chat/completions)."
            )?;
            loop {
                out.flush()?;
                let input = ask_value(ask, "Adres API", default)?;
                if looks_like_secret(&input) {
                    secret_warning(out)?;
                    continue;
                }
                match api::validate_base_url(&input) {
                    Ok(_) => break input,
                    Err(err) => writeln!(out, "{err}"),
                }?;
            }
        }
        None => {
            let base = if reuse_base {
                same_provider.map(|s| s.base_url.as_str())
            } else {
                None
            }
            .unwrap_or_else(|| provider.default_base());
            api::validate_base_url(base)?;
            base.to_owned()
        }
    };
    // A remembered model belongs to a provider AND endpoint, not to another server.
    let matching_server = same_provider.filter(|s| s.base_url == base_url);
    let model = match &cli.model {
        Some(model) => {
            validate_model(model, provider)?;
            model.clone()
        }
        None => {
            writeln!(out, "\n2/7 — Jakiego modelu chcesz używać?")?;
            let examples = example_models(provider, &base_url);
            if examples.is_empty() {
                writeln!(
                    out,
                    "Dla własnego serwera wpisz identyfikator modelu z jego katalogu; musi obsługiwać obrazy."
                )?;
            } else {
                writeln!(
                    out,
                    "Przykładowe modele — wybierz numer albo wpisz własny pełny identyfikator:"
                )?;
                for (index, (id, label)) in examples.iter().enumerate() {
                    writeln!(out, "  {}. {label} — {id}", index + 1)?;
                }
                writeln!(
                    out,
                    "To przykłady, nie sprawdzenie dostępu dla Twojego klucza. Nie wklejaj tutaj klucza API."
                )?;
            }
            let default = matching_server.map(|s| s.model.as_str()).filter(|model| {
                !looks_like_secret(model) && validate_model(model, provider).is_ok()
            });
            if matching_server.is_some() && default.is_none() {
                writeln!(
                    out,
                    "Poprzednia nazwa modelu jest nieprawidłowa — wybierz ją ponownie (dla Claude: prefiks claude-)."
                )?;
            }
            loop {
                out.flush()?;
                let input = ask_value(ask, "Model", default)?;
                if looks_like_secret(&input) {
                    secret_warning(out)?;
                    continue;
                }
                let selected = select_model(&input, examples).and_then(|model| {
                    validate_model(&model, provider)?;
                    Ok(model)
                });
                match selected {
                    Ok(model) => break model,
                    Err(err) => writeln!(out, "{err}"),
                }?;
            }
        }
    };
    let keybind = match &cli.keybind {
        Some(keybind) => configured_keybind(keybind)?,
        None => {
            writeln!(out, "\n3/7 — Jakim skrótem uruchamiać analizę?")?;
            writeln!(
                out,
                "Print = Print Screen. Możesz wpisać np. Ctrl+Shift+F8."
            )?;
            let default = saved.map(|s| s.keybind.as_str()).unwrap_or("Print");
            loop {
                out.flush()?;
                let input = ask_value(ask, "Skrót", Some(default))?;
                if looks_like_secret(&input) {
                    secret_warning(out)?;
                    continue;
                }
                match configured_keybind(&input) {
                    Ok(keybind) => break keybind,
                    // Don't echo invalid input: it might accidentally contain a credential.
                    Err(_) => writeln!(
                        out,
                        "Nieprawidłowy skrót. Wpisz Print albo np. Ctrl+Shift+F8."
                    ),
                }?;
            }
        }
    };
    let pause_keybind = match &cli.pause_keybind {
        Some(pause) => configured_pause_keybind(pause, &keybind)?,
        None => {
            writeln!(out, "\n4/7 — Opcjonalny skrót pauzy i wznowienia")?;
            writeln!(
                out,
                "Np. Ctrl+Shift+F9; brak = bez dodatkowego skrótu. Enter zachowuje wybór."
            )?;
            writeln!(
                out,
                "Pauza przerywa analizę i czyści wynik do myślnika (-), bez innego oznaczenia. Ten sam skrót wznawia; potem naciśnij skrót analizy."
            )?;
            let saved_pause = saved.and_then(|s| s.pause_keybind.as_deref());
            let default = saved_pause.filter(|key| configured_pause_keybind(key, &keybind).is_ok());
            if saved_pause.is_some() && default.is_none() {
                writeln!(
                    out,
                    "Poprzedni skrót pauzy jest nieprawidłowy lub pokrywa się z analizą — wybierz ponownie."
                )?;
            }
            loop {
                out.flush()?;
                let input = ask_value(ask, "Skrót pauzy", Some(default.unwrap_or("brak")))?;
                if looks_like_secret(&input) {
                    secret_warning(out)?;
                    continue;
                }
                match configured_pause_keybind(&input, &keybind) {
                    Ok(pause) => break pause,
                    Err(_) => writeln!(
                        out,
                        "Nieprawidłowy lub powtórzony skrót pauzy. Wpisz inny niż skrót analizy, np. Ctrl+Shift+F9, albo brak."
                    ),
                }?;
            }
        }
    };
    let selection = match cli.selection {
        Some(selection) => selection,
        None => {
            writeln!(out, "\n5/7 — Jedna czy wiele poprawnych odpowiedzi?")?;
            writeln!(
                out,
                "  1. Jedna odpowiedź (single)\n  2. Wiele odpowiedzi (multiple)"
            )?;
            writeln!(
                out,
                "Dla A i B wynik to 1'2. Grupy kropek rozdzielamy tym samym apostrofem."
            )?;
            let default = saved.map(|s| s.selection).unwrap_or_default();
            loop {
                out.flush()?;
                let number = if default == SelectionMode::Single {
                    1
                } else {
                    2
                };
                let input = ask(&format!(
                    "Odpowiedzi [{number} — {}]: ",
                    default.description()
                ))?;
                let input = input.trim();
                if input.is_empty() {
                    break default;
                }
                if looks_like_secret(input) {
                    secret_warning(out)?;
                    continue;
                }
                match input.to_lowercase().as_str() {
                    "1" | "single" | "jedna" => break SelectionMode::Single,
                    "2" | "multiple" | "multi" | "wiele" => break SelectionMode::Multiple,
                    _ => writeln!(out, "Wybierz 1 (jedna) albo 2 (wiele).")?,
                }
            }
        }
    };
    let mode = match cli.mode {
        Some(mode) => mode,
        None => {
            writeln!(out, "\n6/7 — Jak wyświetlać odpowiedź?")?;
            writeln!(out, "  1. Cyfry 1–9 (A–I)\n  2. Kropki 1–6 (A–F)")?;
            writeln!(
                out,
                "Bez tła, obwódki i dymka. Kropki: 5 = jedna nad kwadratem czterech, 6 = trzy rzędy po dwie."
            )?;
            let default = saved.map(|s| s.mode).unwrap_or_default();
            loop {
                out.flush()?;
                let number = if default == AnswerMode::Digits { 1 } else { 2 };
                let input = ask(&format!("Tryb [{number} — {}]: ", default.description()))?;
                let input = input.trim();
                if input.is_empty() {
                    break default;
                }
                if looks_like_secret(input) {
                    secret_warning(out)?;
                    continue;
                }
                match input.to_lowercase().as_str() {
                    "1" | "digits" | "cyfry" | "1-9" => break AnswerMode::Digits,
                    "2" | "dots" | "kropki" | "1-6" => break AnswerMode::Dots,
                    _ => writeln!(out, "Wybierz 1 (cyfry 1–9) albo 2 (kropki 1–6).")?,
                }
            }
        }
    };
    Ok(Settings {
        provider,
        model,
        base_url,
        keybind,
        pause_keybind,
        mode,
        selection,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use std::collections::VecDeque;

    fn saved() -> Settings {
        Settings {
            provider: Provider::Anthropic,
            model: "saved-vision-model".into(),
            base_url: Provider::Anthropic.default_base().into(),
            keybind: "Ctrl+Shift+F8".into(),
            pause_keybind: None,
            mode: AnswerMode::Digits,
            selection: SelectionMode::Single,
        }
    }

    fn run(
        cli: &[&str],
        saved: Option<&Settings>,
        answers: &[&str],
    ) -> (Settings, Vec<String>, String) {
        let cli = Cli::try_parse_from(cli).unwrap();
        let mut answers: VecDeque<_> = answers.iter().copied().collect();
        let mut prompts = Vec::new();
        let mut out = Vec::new();
        let result = configure(
            &cli,
            saved,
            &mut |prompt| {
                prompts.push(prompt.to_owned());
                Ok(answers.pop_front().expect("unexpected prompt").to_owned())
            },
            &mut out,
        )
        .unwrap();
        assert!(answers.is_empty());
        (result, prompts, String::from_utf8(out).unwrap())
    }

    #[test]
    fn selection_and_display_are_ordered_remembered_and_editable() {
        let (settings, prompts, text) = run(
            &["screen-answer"],
            None,
            &["", "vision-model", "", "", "2", "2"],
        );
        assert_eq!(settings.selection, SelectionMode::Multiple);
        assert_eq!(settings.mode, AnswerMode::Dots);
        assert!(prompts[3].starts_with("Skrót pauzy ["));
        assert!(prompts[4].starts_with("Odpowiedzi ["));
        assert!(prompts[5].starts_with("Tryb ["));
        assert!(text.contains("1'2"));
        assert!(text.contains("5 = jedna nad kwadratem czterech"));
        let (again, prompts, _) = run(
            &["screen-answer"],
            Some(&settings),
            &["", "", "", "", "", ""],
        );
        assert_eq!(again.selection, SelectionMode::Multiple);
        assert_eq!(again.mode, AnswerMode::Dots);
        assert!(prompts.last().unwrap().contains("2 — kropki 1–6"));
        let (changed, _, _) = run(
            &["screen-answer"],
            Some(&settings),
            &["", "", "", "", "wrong", "single", "bad", "digits"],
        );
        assert_eq!(changed.selection, SelectionMode::Single);
        assert_eq!(changed.mode, AnswerMode::Digits);
    }

    #[test]
    fn older_settings_default_to_single_digits() {
        let settings: Settings = serde_json::from_str(r#"{"provider":"anthropic","model":"vision-model","base_url":"https://api.anthropic.com/v1","keybind":"Print"}"#).unwrap();
        assert_eq!(settings.mode, AnswerMode::Digits);
        assert_eq!(settings.selection, SelectionMode::Single);
        assert_eq!(settings.pause_keybind, None);
        assert!(
            !serde_json::to_value(&settings)
                .unwrap()
                .as_object()
                .unwrap()
                .contains_key("pause_keybind")
        );
    }

    #[test]
    fn optional_pause_keybind_is_saved_editable_and_can_be_removed() {
        let (settings, _, text) = run(
            &["screen-answer"],
            None,
            &["", "vision-model", "", "Ctrl+Shift+F9", "", ""],
        );
        assert_eq!(settings.pause_keybind.as_deref(), Some("Ctrl+Shift+F9"));
        assert!(text.contains("myślnika (-)"));
        let (again, prompts, _) = run(
            &["screen-answer"],
            Some(&settings),
            &["", "", "", "", "", ""],
        );
        assert_eq!(again.pause_keybind, settings.pause_keybind);
        assert_eq!(prompts[3], "Skrót pauzy [Ctrl+Shift+F9]: ");
        let (cleared, _, _) = run(
            &["screen-answer"],
            Some(&settings),
            &["", "", "", "off", "", ""],
        );
        assert_eq!(cleared.pause_keybind, None);
        let (changed, prompts, text) = run(
            &["screen-answer"],
            Some(&settings),
            &["", "", "Ctrl+Shift+F9", "", "", ""],
        );
        assert_eq!(changed.pause_keybind, None);
        assert_eq!(prompts[3], "Skrót pauzy [brak]: ");
        assert!(text.contains("pokrywa się z analizą"));
    }

    #[test]
    fn invalid_or_duplicate_pause_keys_reprompt_without_echoing_secrets() {
        let (settings, _, text) = run(
            &["screen-answer"],
            None,
            &[
                "",
                "vision-model",
                "Shift+Control+F8",
                "Control+Shift+F8",
                "sk-dummy-not-real",
                "Ctrl++F9",
                "Ctrl+Shift+F9",
                "",
                "",
            ],
        );
        assert_eq!(settings.keybind, "Ctrl+Shift+F8");
        assert_eq!(settings.pause_keybind.as_deref(), Some("Ctrl+Shift+F9"));
        assert!(text.contains("powtórzony skrót pauzy"));
        assert!(!text.contains("sk-dummy-not-real"));
        assert!(!text.contains("Ctrl++F9"));
        assert!(configured_pause_keybind("Ctrl+F8", "Control+F8").is_err());
        assert!(configured_pause_keybind("sk-dummy-not-real", "Print").is_err());
    }

    #[test]
    fn wizard_selects_models_by_number_for_every_provider() {
        for (number, provider, choice, expected) in [
            ("1", Provider::Anthropic, "1", "claude-haiku-5-5"),
            ("1", Provider::Anthropic, "2", "claude-sonnet-5-5"),
            ("1", Provider::Anthropic, "3", "claude-opus-5-5"),
            ("1", Provider::Anthropic, "4", "claude-fable-5-2"),
            ("1", Provider::Anthropic, "5", "claude-sonnet-4-6"),
            ("2", Provider::Openai, "1", "gpt-5.4-mini"),
            ("2", Provider::Openai, "6", "gpt-6.2-astra"),
            ("3", Provider::Openrouter, "1", "anthropic/claude-haiku-5.5"),
            (
                "3",
                Provider::Openrouter,
                "2",
                "anthropic/claude-sonnet-5.5",
            ),
            ("3", Provider::Openrouter, "3", "anthropic/claude-opus-5.5"),
            ("3", Provider::Openrouter, "9", "google/gemini-4-carbon"),
            ("4", Provider::Gemini, "1", "gemini-3.5-flash-lite"),
            ("4", Provider::Gemini, "8", "gemini-4-carbon"),
        ] {
            let (settings, _, text) =
                run(&["screen-answer"], None, &[number, choice, "", "", "", ""]);
            assert_eq!(settings.provider, provider);
            assert_eq!(settings.base_url, provider.default_base());
            assert_eq!(settings.model, expected);
            for (index, (id, label)) in example_models(provider, &settings.base_url)
                .iter()
                .enumerate()
            {
                assert!(text.contains(&format!("{}. {label} — {id}", index + 1)));
            }
        }
    }

    #[test]
    fn typo_is_rejected_and_bad_saved_default_is_not_offered() {
        let saved = Settings {
            model: "claud-opus-5-5".into(),
            ..saved()
        };
        let (settings, prompts, text) = run(
            &["screen-answer"],
            Some(&saved),
            &["", "claud-opus-5-5", "3", "", "", "", ""],
        );
        assert_eq!(settings.model, "claude-opus-5-5");
        assert_eq!(prompts[1], "Model: ");
        assert!(text.contains("Literówka"));
        assert!(text.contains("Poprzednia nazwa modelu jest nieprawidłowa"));
    }

    #[test]
    fn model_numbers_are_checked_and_ids_can_be_entered_directly() {
        for provider in [
            Provider::Anthropic,
            Provider::Openai,
            Provider::Openrouter,
            Provider::Gemini,
        ] {
            let examples = example_models(provider, provider.default_base());
            assert!(examples.len() >= 4, "{provider:?} needs multiple examples");
            let mut ids = std::collections::HashSet::new();
            for (index, (id, label)) in examples.iter().enumerate() {
                assert!(ids.insert(id), "Duplicate model: {id}");
                assert!(!label.is_empty());
                assert_eq!(
                    select_model(&(index + 1).to_string(), examples).unwrap(),
                    *id
                );
                validate_model(id, provider).unwrap();
                api::endpoint_url(provider, id, provider.default_base()).unwrap();
            }
            let past_end = (examples.len() + 1).to_string();
            for input in ["0", past_end.as_str(), "99999999999999999999999999999999"] {
                assert!(select_model(input, examples).is_err());
            }
            assert_eq!(
                select_model("my-future-model", examples).unwrap(),
                "my-future-model"
            );
            assert!(example_models(provider, "http://localhost:8000/v1").is_empty());
        }
        assert_eq!(select_model("2", &[]).unwrap(), "2");
    }

    #[test]
    fn wizard_defaults_saved_choices_and_edits() {
        let saved = saved();
        for (previous, answers, model, keybind, model_prompt) in [
            (
                None,
                ["", "vision-model", "", "", "", ""],
                "vision-model",
                "Print",
                "Model: ",
            ),
            (
                Some(&saved),
                ["", "", "", "", "", ""],
                "saved-vision-model",
                "Ctrl+Shift+F8",
                "Model [saved-vision-model]: ",
            ),
            (
                Some(&saved),
                ["claude", "new-model", "PrintScreen", "", "", ""],
                "new-model",
                "Print",
                "Model [saved-vision-model]: ",
            ),
        ] {
            let (settings, prompts, text) = run(&["screen-answer"], previous, &answers);
            assert_eq!(settings.provider, Provider::Anthropic);
            assert_eq!(settings.model, model);
            assert_eq!(settings.keybind, keybind);
            assert_eq!(settings.mode, AnswerMode::Digits);
            assert_eq!(settings.selection, SelectionMode::Single);
            assert_eq!(prompts.len(), 6);
            assert_eq!(prompts[1], model_prompt);
            assert!(text.contains("9 opcji (A–I)"));
        }
    }

    #[test]
    fn changing_provider_resets_endpoint_and_model_default() {
        let (settings, prompts, _) = run(
            &["screen-answer"],
            Some(&saved()),
            &["4", "gemini-model", "", "", "", ""],
        );
        assert_eq!(settings.provider, Provider::Gemini);
        assert_eq!(settings.base_url, Provider::Gemini.default_base());
        assert_eq!(prompts[1], "Model: ");
    }

    #[test]
    fn custom_endpoint_is_configurable_without_flags() {
        let (settings, _, _) = run(
            &["screen-answer"],
            None,
            &[
                "5",
                "http://remote.example/v1",
                "http://localhost:8000/v1",
                "local-model",
                "",
                "",
                "",
                "",
            ],
        );
        assert_eq!(settings.provider, Provider::Openai);
        assert_eq!(settings.base_url, "http://localhost:8000/v1");
        assert_eq!(settings.model, "local-model");
    }

    #[test]
    fn invalid_inputs_reprompt_without_echoing_secrets() {
        let (settings, prompts, text) = run(
            &["screen-answer"],
            None,
            &[
                "sk-dummy-not-real",
                "bad-provider",
                "1",
                "",
                "sk-dummy-not-real",
                "vision-model",
                "Bad+F8",
                "Print",
                "",
                "",
                "",
            ],
        );
        assert_eq!(settings.model, "vision-model");
        assert_eq!(prompts.len(), 11);
        assert!(!text.contains("sk-dummy-not-real"));
        assert!(text.contains("unieważnij"));
    }

    #[test]
    fn explicit_flags_skip_their_questions() {
        let (settings, prompts, _) = run(
            &[
                "screen-answer",
                "--provider",
                "openai",
                "--model",
                "flag-model",
                "--base-url",
                "http://localhost:8000/v1",
                "--keybind",
                "Ctrl+F8",
                "--pause-keybind",
                "Ctrl+F9",
                "--mode",
                "digits",
                "--selection",
                "single",
            ],
            Some(&saved()),
            &[],
        );
        assert!(prompts.is_empty());
        assert_eq!(settings.keybind, "Ctrl+F8");
        assert_eq!(settings.pause_keybind.as_deref(), Some("Ctrl+F9"));
        assert_eq!(settings.model, "flag-model");
    }

    #[test]
    fn changing_custom_server_does_not_reuse_model() {
        let saved = Settings {
            provider: Provider::Openai,
            model: "server-one-model".into(),
            base_url: "http://localhost:8000/v1".into(),
            keybind: "Print".into(),
            pause_keybind: None,
            mode: AnswerMode::Digits,
            selection: SelectionMode::Single,
        };
        let (settings, prompts, _) = run(
            &["screen-answer"],
            Some(&saved),
            &[
                "",
                "http://localhost:9000/v1",
                "server-two-model",
                "",
                "",
                "",
                "",
            ],
        );
        assert_eq!(prompts[2], "Model: ");
        assert_eq!(settings.model, "server-two-model");
    }
}
