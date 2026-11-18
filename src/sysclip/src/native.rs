//! The native clipboard.

use crate::ClipboardError;

pub(crate) trait Native {
    fn read(&mut self) -> Result<String, ClipboardError>;
    fn write(&mut self, text: &str) -> Result<(), ClipboardError>;
}

pub(crate) struct Arboard(arboard::Clipboard);

impl Arboard {
    pub(crate) fn open() -> Result<Self, ClipboardError> {
        arboard::Clipboard::new()
            .map(Self)
            .map_err(|cause| ClipboardError::new(&cause))
    }
}

impl Native for Arboard {
    fn read(&mut self) -> Result<String, ClipboardError> {
        read_text(self.0.get_text())
    }

    fn write(&mut self, text: &str) -> Result<(), ClipboardError> {
        self.0
            .set_text(text)
            .map_err(|cause| ClipboardError::new(&cause))
    }
}

fn read_text(_result: Result<String, arboard::Error>) -> Result<String, ClipboardError> {
    Err(ClipboardError::new(&"placeholder"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_text_passes_text_through() {
        assert_eq!(read_text(Ok("日本語 café".to_string())), Ok("日本語 café".to_string()));
    }

    #[test]
    fn read_text_gives_an_empty_string_when_no_text_is_held() {
        assert_eq!(
            read_text(Err(arboard::Error::ContentNotAvailable)),
            Ok(String::new())
        );
    }

    #[test]
    fn read_text_names_an_unsupported_clipboard_without_a_period() {
        let error = read_text(Err(arboard::Error::ClipboardNotSupported)).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("not supported"), "{message}");
        assert!(!message.ends_with('.'), "{message}");
    }

    #[test]
    fn read_text_carries_the_description_of_an_unknown_error() {
        let error = read_text(Err(arboard::Error::Unknown {
            description: "the pasteboard is gone.".to_string(),
        }))
        .unwrap_err();
        assert_eq!(error.to_string(), "the pasteboard is gone");
    }
}
