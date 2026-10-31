//! The system spelling checker, `NSSpellChecker`, as TextEdit and Chromium on macOS use it: the
//! user's languages (automatic by default) and learned words, which Learn spelling adds to.

use objc2_app_kit::NSSpellChecker;
use objc2_foundation::{NSRange, NSString};
use std::sync::Mutex;

pub struct SystemSpeller {
    /// The spell document all pages share, so the checker can keep per-document state.
    tag: isize,
}

/// One caller at a time: background checks and the menu may otherwise reach the shared
/// checker at once.
static CHECKER: Mutex<()> = Mutex::new(());

impl SystemSpeller {
    pub fn new() -> Self {
        Self {
            tag: NSSpellChecker::uniqueSpellDocumentTag(),
        }
    }

    fn with<T>(&self, run: impl FnOnce(&NSSpellChecker) -> T) -> T {
        let _guard = CHECKER
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        objc2::rc::autoreleasepool(|_| run(&NSSpellChecker::sharedSpellChecker()))
    }
}

impl super::Speller for SystemSpeller {
    fn misspelled(&self, words: &[&str]) -> Vec<bool> {
        self.with(|checker| {
            words
                .iter()
                .map(|word| {
                    // SAFETY: a null word count is allowed, and the string outlives the call.
                    let found = unsafe {
                        checker
                            .checkSpellingOfString_startingAt_language_wrap_inSpellDocumentWithTag_wordCount(
                                &NSString::from_str(word),
                                0,
                                None,
                                false,
                                self.tag,
                                std::ptr::null_mut(),
                            )
                    };
                    found.length > 0
                })
                .collect()
        })
    }

    fn guesses(&self, word: &str) -> Vec<String> {
        self.with(|checker| {
            let string = NSString::from_str(word);
            checker
                .guessesForWordRange_inString_language_inSpellDocumentWithTag(
                    NSRange::new(0, string.length()),
                    &string,
                    None,
                    self.tag,
                )
                .map(|guesses| guesses.iter().map(|guess| guess.to_string()).collect())
                .unwrap_or_default()
        })
    }

    fn learn(&self, word: &str) {
        self.with(|checker| checker.learnWord(&NSString::from_str(word)));
    }
}
