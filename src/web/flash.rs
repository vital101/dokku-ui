use actix_session::Session;
use serde::{Deserialize, Serialize};

const FLASH_KEY: &str = "flash";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FlashLevel {
    Success,
    Error,
    Info,
}

impl FlashLevel {
    pub fn css_class(self) -> &'static str {
        match self {
            FlashLevel::Success => "bg-emerald-500/10 text-emerald-400 border-emerald-500/30",
            FlashLevel::Error => "bg-red-500/10 text-red-400 border-red-500/30",
            FlashLevel::Info => "bg-sky-500/10 text-sky-400 border-sky-500/30",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlashMessage {
    pub level: FlashLevel,
    pub message: String,
}

pub fn set_flash(session: &Session, level: FlashLevel, message: impl Into<String>) {
    let _ = session.insert(
        FLASH_KEY,
        FlashMessage {
            level,
            message: message.into(),
        },
    );
}

pub fn take_flash(session: &Session) -> Option<FlashMessage> {
    let flash = session.get::<FlashMessage>(FLASH_KEY).ok().flatten();
    if flash.is_some() {
        session.remove(FLASH_KEY);
    }
    flash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_css_classes_are_distinct() {
        let mut classes = vec![
            FlashLevel::Success.css_class(),
            FlashLevel::Error.css_class(),
            FlashLevel::Info.css_class(),
        ];
        classes.sort();
        classes.dedup();
        assert_eq!(classes.len(), 3);
    }
}
