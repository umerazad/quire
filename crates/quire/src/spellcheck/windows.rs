//! The system spelling checker on Windows, the Spell Checking API (`ISpellChecker`) that Edge,
//! Chromium, and the built-in text controls use: the user's display language when it has a
//! dictionary, else US English, and the user's dictionary, which Learn spelling adds to. The
//! checker lives on its own thread in the multithreaded COM apartment, so background checks and
//! the window thread's menu reach it without crossing apartments.

use std::sync::mpsc;
use windows::{
    Win32::{
        Globalization::{
            GetUserDefaultLocaleName, ISpellChecker, ISpellCheckerFactory, ISpellingError,
            SpellCheckerFactory,
        },
        System::Com::{
            CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
            CoTaskMemFree,
        },
    },
    core::{HSTRING, PWSTR},
};

enum Request {
    Misspelled(Vec<String>, mpsc::Sender<Vec<bool>>),
    Guesses(String, mpsc::Sender<Vec<String>>),
    Learn(String),
}

pub struct SystemSpeller {
    requests: mpsc::Sender<Request>,
}

const FALLBACK_LANGUAGE: &str = "en-US";

fn user_language() -> String {
    let mut name = [0u16; 85];
    // SAFETY: the buffer is LOCALE_NAME_MAX_LENGTH (85) characters, as the call requires.
    let length = unsafe { GetUserDefaultLocaleName(&mut name) };
    match usize::try_from(length) {
        Ok(length) if length > 1 => String::from_utf16_lossy(&name[..length - 1]),
        _ => FALLBACK_LANGUAGE.into(),
    }
}

fn open_checker() -> windows::core::Result<ISpellChecker> {
    // SAFETY: COM is initialized on this thread before any call, and every pointer passed is a
    // live HSTRING for the length of the call.
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
        let factory: ISpellCheckerFactory =
            CoCreateInstance(&SpellCheckerFactory, None, CLSCTX_INPROC_SERVER)?;
        let preferred = HSTRING::from(user_language());
        let language = if factory.IsSupported(&preferred)?.as_bool() {
            preferred
        } else {
            HSTRING::from(FALLBACK_LANGUAGE)
        };
        factory.CreateSpellChecker(&language)
    }
}

fn misspelled(checker: &ISpellChecker, word: &str) -> bool {
    // SAFETY: the word outlives the call; `Next` writes at most one error into `error`.
    unsafe {
        let Ok(errors) = checker.Check(&HSTRING::from(word)) else {
            return false;
        };
        let mut error: Option<ISpellingError> = None;
        errors.Next(&mut error).is_ok() && error.is_some()
    }
}

fn guesses(checker: &ISpellChecker, word: &str) -> Vec<String> {
    let mut found = Vec::new();
    // SAFETY: each string `Next` hands back is read once and then freed with CoTaskMemFree, as
    // the enumerator's contract requires.
    unsafe {
        let Ok(suggestions) = checker.Suggest(&HSTRING::from(word)) else {
            return found;
        };
        loop {
            let mut next = [PWSTR::null()];
            let mut fetched = 0u32;
            if suggestions.Next(&mut next, Some(&mut fetched)).is_err() || fetched == 0 {
                break;
            }
            if let Ok(guess) = next[0].to_string() {
                found.push(guess);
            }
            CoTaskMemFree(Some(next[0].0 as *const _));
        }
    }
    found
}

fn serve(checker: ISpellChecker, requests: mpsc::Receiver<Request>) {
    for request in requests {
        match request {
            Request::Misspelled(words, reply) => {
                reply
                    .send(
                        words
                            .iter()
                            .map(|word| misspelled(&checker, word))
                            .collect(),
                    )
                    .ok();
            }
            Request::Guesses(word, reply) => {
                reply.send(guesses(&checker, &word)).ok();
            }
            Request::Learn(word) => {
                // SAFETY: the word outlives the call.
                unsafe { checker.Add(&HSTRING::from(word.as_str())) }.ok();
            }
        }
    }
}

impl SystemSpeller {
    /// Starts the checker thread, or None when Windows has no spell checker for the user's
    /// language or US English.
    pub fn new() -> Option<Self> {
        let (send, requests) = mpsc::channel();
        let (opened, ready) = mpsc::channel();
        std::thread::Builder::new()
            .name("QuireSpelling".into())
            .spawn(move || match open_checker() {
                Ok(checker) => {
                    opened.send(true).ok();
                    serve(checker, requests);
                }
                Err(_) => {
                    opened.send(false).ok();
                }
            })
            .ok()?;
        ready
            .recv()
            .unwrap_or(false)
            .then(|| Self { requests: send })
    }

    fn send(&self, request: Request) {
        self.requests.send(request).ok();
    }
}

impl super::Speller for SystemSpeller {
    fn misspelled(&self, words: &[&str]) -> Vec<bool> {
        let (reply, answer) = mpsc::channel();
        self.send(Request::Misspelled(
            words.iter().map(|word| (*word).to_owned()).collect(),
            reply,
        ));
        answer.recv().unwrap_or_else(|_| vec![false; words.len()])
    }

    fn guesses(&self, word: &str) -> Vec<String> {
        let (reply, answer) = mpsc::channel();
        self.send(Request::Guesses(word.to_owned(), reply));
        answer.recv().unwrap_or_default()
    }

    fn learn(&self, word: &str) {
        self.send(Request::Learn(word.to_owned()));
    }
}
