use anyhow::{Result, bail};

/// Nonempty, unique choices 1–9 in canonical ascending order. No heap allocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Choices(u16);

impl Choices {
    pub fn new(values: &[u8]) -> Result<Self> {
        if values.is_empty() || values.len() > 9 {
            bail!("Oczekiwano od jednej do dziewięciu różnych odpowiedzi.");
        }
        let mut mask = 0u16;
        for &value in values {
            if !(1..=9).contains(&value) {
                bail!("Odpowiedź jest poza zakresem 1–9.");
            }
            let bit = 1u16 << (value - 1);
            if mask & bit != 0 {
                bail!("Model powtórzył tę samą odpowiedź.");
            }
            mask |= bit;
        }
        Ok(Self(mask))
    }

    pub fn iter(self) -> impl Iterator<Item = u8> {
        (1..=9).filter(move |n| self.0 & (1 << (n - 1)) != 0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Choice(u8),
    Multiple(Choices),
    Unknown,
}

impl Answer {
    pub fn values(self) -> Vec<u8> {
        match self {
            Self::Choice(n) => vec![n],
            Self::Multiple(choices) => choices.iter().collect(),
            Self::Unknown => Vec::new(),
        }
    }
}

pub fn parse_answer(text: &str) -> Result<Answer> {
    let text = text.trim();
    match text.as_bytes() {
        [b'1'..=b'9'] => Ok(Answer::Choice(text.as_bytes()[0] - b'0')),
        [b'?'] => Ok(Answer::Unknown),
        _ => bail!("Model zwrócił nieprawidłowy format odpowiedzi (oczekiwano 1–9 albo ?)."),
    }
}

pub fn parse_multiple_answer(text: &str) -> Result<Answer> {
    let text = text.trim();
    if text == "?" {
        return Ok(Answer::Unknown);
    }
    let values: Result<Vec<_>> = text
        .split('\'')
        .map(|part| match part.as_bytes() {
            [b'1'..=b'9'] => Ok(part.as_bytes()[0] - b'0'),
            _ => bail!("Nieprawidłowy format wielu odpowiedzi (oczekiwano np. 1'2 albo ?)."),
        })
        .collect();
    Ok(Answer::Multiple(Choices::new(&values?)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_answer_requires_one_exact_digit_or_unknown() {
        for n in 1..=9 {
            assert_eq!(parse_answer(&format!(" {n}\n")).unwrap(), Answer::Choice(n));
        }
        assert_eq!(parse_answer("?").unwrap(), Answer::Unknown);
        for bad in [
            "",
            "0",
            "10",
            "A",
            "1 2",
            "1\n2",
            "Answer: 3",
            "**4**",
            "９",
            "2.",
            "1 or 2",
        ] {
            assert!(parse_answer(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn multiple_answers_are_sorted_and_separated_only_by_apostrophes() {
        assert_eq!(
            parse_multiple_answer(" 3'1'2\n").unwrap().values(),
            vec![1, 2, 3]
        );
        assert_eq!(parse_multiple_answer("9").unwrap().values(), vec![9]);
        assert_eq!(parse_multiple_answer("?").unwrap(), Answer::Unknown);
        assert_eq!(
            Choices::new(&[9, 1, 5]).unwrap().iter().collect::<Vec<_>>(),
            vec![1, 5, 9]
        );
        for bad in [
            "",
            "0",
            "10",
            "A'B",
            "1,2",
            "1 2",
            "1 ' 2",
            "1''2",
            "'1",
            "1'",
            "1'1",
            "1'?",
            "1’2",
            "**1'2**",
            "Answer: 1'2",
            "[1,2]",
        ] {
            assert!(parse_multiple_answer(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn no_empty_duplicate_or_out_of_range_choices() {
        for values in [vec![], vec![0], vec![10], vec![1, 1], vec![1, 2, 1]] {
            assert!(Choices::new(&values).is_err());
        }
        assert_eq!(
            Choices::new(&[1, 2, 3, 4, 5, 6, 7, 8, 9])
                .unwrap()
                .iter()
                .count(),
            9
        );
    }
}
