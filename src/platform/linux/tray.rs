//! Linux StatusNotifierItem adapter; rasterization and layout are shared with Windows.
pub use crate::display::State;
use crate::{
    display,
    mode::{AnswerMode, SelectionMode},
};
use anyhow::Result;
use ksni::{Icon, Tray};

pub struct Indicator {
    pub state: State,
    pub mode: AnswerMode,
    pub slot: usize,
    pub visible: bool,
}

pub struct Bar {
    mode: AnswerMode,
    cells: super::sni::Cells,
}

impl Bar {
    pub async fn spawn(mode: AnswerMode, selection: SelectionMode) -> Result<Self> {
        let count = if selection == SelectionMode::Multiple {
            mode.max_choices() as usize
        } else {
            1
        };
        let cells = super::sni::Cells::spawn(mode, count).await?;
        Ok(Self { mode, cells })
    }

    pub async fn update(&self, state: State) -> Result<()> {
        let states = display::cell_states(state, self.mode, self.cells.len());
        for slot in 1..self.cells.len() {
            self.cells.update(slot, State::Idle, false).await?;
        }
        for (slot, state) in states.into_iter().enumerate() {
            if let Some(state) = state {
                self.cells.update(slot, state, true).await?;
            }
        }
        Ok(())
    }

    pub async fn shutdown(&self) {
        self.cells.shutdown().await;
    }
}

impl Tray for Indicator {
    fn id(&self) -> String {
        if self.slot == 0 {
            "screen-answer".into()
        } else {
            format!("screen-answer-{:02}", self.slot + 1)
        }
    }
    fn status(&self) -> ksni::Status {
        if self.visible {
            ksni::Status::Active
        } else {
            ksni::Status::Passive
        }
    }
    fn title(&self) -> String {
        String::new()
    }
    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip::default()
    }
    fn icon_pixmap(&self) -> Vec<Icon> {
        let image = display::render_cell(self.state, self.mode, self.slot > 0);
        vec![Icon {
            width: image.width,
            height: image.height,
            data: image.data,
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unused_cells_are_passive_and_have_stable_ids() {
        let first = Indicator {
            state: State::Idle,
            mode: AnswerMode::Dots,
            slot: 0,
            visible: true,
        };
        let next = Indicator {
            state: State::Idle,
            mode: AnswerMode::Dots,
            slot: 1,
            visible: false,
        };
        assert_eq!(first.id(), "screen-answer");
        assert_eq!(next.id(), "screen-answer-02");
        assert!(matches!(next.status(), ksni::Status::Passive));
    }
    #[test]
    fn no_hover_text_or_fallback_title() {
        let indicator = Indicator {
            state: State::Idle,
            mode: AnswerMode::Digits,
            slot: 0,
            visible: true,
        };
        assert!(indicator.title().is_empty());
        let tip = indicator.tool_tip();
        assert!(
            tip.title.is_empty()
                && tip.description.is_empty()
                && tip.icon_name.is_empty()
                && tip.icon_pixmap.is_empty()
        );
    }
}
