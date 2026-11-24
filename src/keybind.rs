use anyhow::{Context, Result, bail};

/// Common spelling; platform-specific code validates the actual supported key symbols.
pub fn normalize_keybind(input: &str) -> Result<String> {
    let mut parts: Vec<&str> = input.split('+').collect();
    let key = parts.pop().context("Pusty skrót.")?;
    let key = match key.to_ascii_lowercase().as_str() {
        "printscreen" | "print-screen" | "prtsc" | "print" => "Print",
        _ => key,
    };
    if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        bail!("Nieprawidłowy klawisz. Przykład: Print lub Ctrl+Shift+F8.");
    }
    let mut modifiers = Vec::new();
    for modifier in parts {
        let m = match modifier.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => "Ctrl",
            "shift" => "Shift",
            "alt" => "Alt",
            "super" | "win" => "Super",
            "mod" => "Mod",
            _ => bail!("Nieznany modyfikator skrótu: {modifier}"),
        };
        if modifiers.contains(&m) {
            bail!("Powtórzony modyfikator skrótu.");
        }
        modifiers.push(m);
    }
    modifiers.push(key);
    Ok(modifiers.join("+"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn keybind_validation() {
        assert_eq!(normalize_keybind("PrintScreen").unwrap(), "Print");
        assert_eq!(
            normalize_keybind("control+shift+F8").unwrap(),
            "Ctrl+Shift+F8"
        );
        for bad in [
            "",
            "Ctrl+",
            "A;evil",
            "Print\n",
            "Ctrl+Ctrl+A",
            "Foo+F8",
            "\"Print\"",
        ] {
            assert!(normalize_keybind(bad).is_err());
        }
    }
}
