//! Platform-independent answer layout and transparent ARGB32 pixels.
use crate::{answer::Answer, mode::AnswerMode};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Idle,
    Busy,
    Answer(Answer),
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Icon {
    pub width: i32,
    pub height: i32,
    pub data: Vec<u8>,
}

/// Every update replaces all cells; no stale trailing answers can survive a new question.
pub fn cell_states(mut state: State, mode: AnswerMode, count: usize) -> Vec<Option<State>> {
    if count == 0 {
        return Vec::new();
    }
    let mut values = match state {
        State::Answer(answer) => answer.values(),
        _ => Vec::new(),
    };
    if values.len() > count || values.iter().any(|n| *n == 0 || *n > mode.max_choices()) {
        state = State::Error;
        values.clear();
    }
    let mut cells = vec![None; count];
    cells[0] = Some(
        values
            .first()
            .map(|n| State::Answer(Answer::Choice(*n)))
            .unwrap_or(state),
    );
    for (slot, &value) in values.iter().enumerate().skip(1) {
        cells[slot] = Some(State::Answer(Answer::Choice(value)));
    }
    cells
}

pub fn render_cell(state: State, mode: AnswerMode, prefixed: bool) -> Icon {
    let mut icon = render(state, mode);
    if prefixed {
        add_apostrophe(&mut icon);
    }
    icon
}

fn glyph(c: char) -> [u8; 7] {
    match c {
        '1' => [4, 12, 4, 4, 4, 4, 14],
        '2' => [14, 17, 1, 2, 4, 8, 31],
        '3' => [30, 1, 1, 14, 1, 1, 30],
        '4' => [2, 6, 10, 18, 31, 2, 2],
        '5' => [31, 16, 16, 30, 1, 1, 30],
        '6' => [14, 16, 16, 30, 17, 17, 14],
        '7' => [31, 1, 2, 4, 8, 8, 8],
        '8' => [14, 17, 17, 14, 17, 17, 14],
        '9' => [14, 17, 17, 15, 1, 1, 14],
        '?' => [14, 17, 1, 2, 4, 0, 4],
        '!' => [4, 4, 4, 4, 4, 0, 4],
        '…' => [0, 0, 0, 0, 0, 0, 21],
        _ => [0, 0, 0, 31, 0, 0, 0],
    }
}

fn dot_centers(n: u8) -> &'static [(i32, i32)] {
    match n {
        1 => &[(16, 16)],
        2 => &[(10, 16), (22, 16)],
        3 => &[(16, 10), (10, 22), (22, 22)],
        4 => &[(10, 10), (22, 10), (10, 22), (22, 22)],
        5 => &[(16, 6), (10, 16), (22, 16), (10, 26), (22, 26)],
        6 => &[(10, 6), (22, 6), (10, 16), (22, 16), (10, 26), (22, 26)],
        _ => &[],
    }
}

fn render(state: State, mode: AnswerMode) -> Icon {
    let mut data = vec![0u8; 32 * 32 * 4];
    if let State::Answer(Answer::Choice(n)) = state
        && mode == AnswerMode::Dots
        && (1..=6).contains(&n)
    {
        for &(cx, cy) in dot_centers(n) {
            for y in cy - 3..=cy + 3 {
                for x in cx - 3..=cx + 3 {
                    if (x - cx).pow(2) + (y - cy).pow(2) <= 9 {
                        let i = (y as usize * 32 + x as usize) * 4;
                        data[i..i + 4].copy_from_slice(&[255; 4]);
                    }
                }
            }
        }
    } else {
        let symbol = match state {
            State::Idle => '-',
            State::Busy => '…',
            State::Error => '!',
            State::Answer(Answer::Unknown) => '?',
            State::Answer(Answer::Choice(n)) if (1..=mode.max_choices()).contains(&n) => {
                char::from(b'0' + n)
            }
            _ => '!',
        };
        for (row, bits) in glyph(symbol).iter().enumerate() {
            for col in 0..5 {
                if bits & (1 << (4 - col)) != 0 {
                    for dy in 0..3 {
                        for dx in 0..3 {
                            let i = ((5 + row * 3 + dy) * 32 + 9 + col * 3 + dx) * 4;
                            data[i..i + 4].copy_from_slice(&[255; 4]);
                        }
                    }
                }
            }
        }
    }
    Icon {
        width: 32,
        height: 32,
        data,
    }
}

fn add_apostrophe(icon: &mut Icon) {
    for y in 0..32 {
        for x in (4..32).rev() {
            let src = (y * 32 + x - 4) * 4;
            let dst = (y * 32 + x) * 4;
            icon.data.copy_within(src..src + 4, dst);
        }
        icon.data[y * 32 * 4..(y * 32 + 4) * 4].fill(0);
    }
    for y in 5..11 {
        for x in 3..=if y < 9 { 5 } else { 4 } {
            let i = (y * 32 + x) * 4;
            icon.data[i..i + 4].copy_from_slice(&[255; 4]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::answer::Choices;
    use std::collections::HashSet;

    fn components(icon: &Icon) -> usize {
        let mut points: HashSet<_> = icon
            .data
            .chunks_exact(4)
            .enumerate()
            .filter(|(_, p)| p[0] > 0)
            .map(|(i, _)| (i as i32 % 32, i as i32 / 32))
            .collect();
        let mut count = 0;
        while let Some(&start) = points.iter().next() {
            count += 1;
            let mut todo = vec![start];
            while let Some((x, y)) = todo.pop() {
                if points.remove(&(x, y)) {
                    todo.extend([(x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)]);
                }
            }
        }
        count
    }

    #[test]
    fn digits_and_statuses_are_distinct_and_transparent() {
        let mut images: Vec<_> = (1..=9)
            .map(|n| render_cell(State::Answer(Answer::Choice(n)), AnswerMode::Digits, false))
            .collect();
        for state in [
            State::Idle,
            State::Busy,
            State::Error,
            State::Answer(Answer::Unknown),
        ] {
            images.push(render_cell(state, AnswerMode::Digits, false));
        }
        for (i, image) in images.iter().enumerate() {
            assert_eq!(
                (image.width, image.height, image.data.len()),
                (32, 32, 4096)
            );
            assert!(
                image
                    .data
                    .chunks_exact(4)
                    .all(|p| p == [0; 4] || p == [255; 4])
            );
            assert_eq!(&image.data[..4], &[0; 4]);
            for other in &images[i + 1..] {
                assert_ne!(image.data, other.data);
            }
        }
    }

    #[test]
    fn agreed_dot_layout_and_separator_do_not_change() {
        assert_eq!(
            dot_centers(5),
            &[(16, 6), (10, 16), (22, 16), (10, 26), (22, 26)]
        );
        assert_eq!(
            dot_centers(6),
            &[(10, 6), (22, 6), (10, 16), (22, 16), (10, 26), (22, 26)]
        );
        for mode in [AnswerMode::Digits, AnswerMode::Dots] {
            for n in 1..=mode.max_choices() {
                let a = render_cell(State::Answer(Answer::Choice(n)), mode, false);
                let b = render_cell(State::Answer(Answer::Choice(n)), mode, true);
                if mode == AnswerMode::Dots {
                    assert_eq!(components(&a), n as usize);
                }
                assert_eq!(components(&b), components(&a) + 1);
                assert_eq!(
                    b.data.chunks_exact(4).filter(|p| p[0] > 0).count(),
                    a.data.chunks_exact(4).filter(|p| p[0] > 0).count() + 16
                );
                assert!(b.data.chunks_exact(4).all(|p| p == [0; 4] || p == [255; 4]));
            }
        }
    }

    #[test]
    fn cell_updates_clear_old_answers_and_reject_invalid_or_oversized_sets() {
        let multi = State::Answer(Answer::Multiple(Choices::new(&[1, 3, 6]).unwrap()));
        let cells = cell_states(multi, AnswerMode::Dots, 6);
        assert_eq!(
            cells[..3],
            [
                Some(State::Answer(Answer::Choice(1))),
                Some(State::Answer(Answer::Choice(3))),
                Some(State::Answer(Answer::Choice(6)))
            ]
        );
        assert!(cells[3..].iter().all(Option::is_none));
        for state in [
            State::Busy,
            State::Idle,
            State::Error,
            State::Answer(Answer::Unknown),
        ] {
            let cells = cell_states(state, AnswerMode::Dots, 6);
            assert_eq!(cells[0], Some(state));
            assert!(cells[1..].iter().all(Option::is_none));
        }
        for state in [
            multi,
            State::Answer(Answer::Choice(0)),
            State::Answer(Answer::Choice(7)),
        ] {
            assert_eq!(
                cell_states(state, AnswerMode::Dots, 1),
                vec![Some(State::Error)]
            );
        }
        assert!(cell_states(State::Idle, AnswerMode::Digits, 0).is_empty());
    }
}
