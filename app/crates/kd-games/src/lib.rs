//! The games Kotodama works with (engine/games/). Each is a module with a type implementing [`Game`]; [`games`] lists
//! them in the window's game picker. A new game: a new module here (its link: how the game tells Kotodama whom the
//! player hears and asks for the microphone, and how Kotodama hands the game what the player said), added to games().
//!
//! The engine (speech to text, the voice mixer) knows no game; a game module is only its LINK. The game's state, as
//! the module reads it, is a FEED (kd_common::feed::Feed), handed to the mixer (a FeedSink) and kept by the module.
pub mod steam;
pub mod teardown;

use kd_common::feed::{Feed, FeedSink};
use kd_common::Log;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

/// What every game module gives the program (engine/games/base.py).
pub trait Game: Send + Sync {
    /// short, for settings: "teardown"
    fn id(&self) -> &'static str;
    /// shown in the game picker: "Teardown"
    fn name(&self) -> &'static str;
    /// what the player needs in the game, shown while waiting: "the Proximity Babble Chat mod"
    fn needs(&self) -> &'static str;

    // ---- the program calls these
    /// (found, where): is the game installed here, and where its files are (shown in the app)
    fn locate(&self) -> (bool, String) {
        (false, String::new())
    }

    /// start listening to the game (a thread of its own); tell it Kotodama runs
    fn start(&mut self) {}

    /// stop; tell the game Kotodama is gone
    fn stop(&mut self) {}

    /// hand the game what the player said: kind 's' (they started talking), 'l' (the words so far, only ever
    /// growing), 'f' (the finished line; "" = nothing made out). times: each unit's start in s after t0 (when the
    /// line's audio began). False if no game is listening
    fn send(&self, _kind: char, _utt: u32, _text: &str, _times: Option<&[f64]>, _t0: Option<Instant>) -> bool {
        false
    }

    /// a typed line (--type, --auto): handed to the game as a finished line
    fn send_text(&self, _text: &str) -> bool {
        false
    }

    /// {src: the wav file of that test voice}: the recorded voices the game's test speakers play (the program loads
    /// them with kd_audio::load_wav)
    fn test_voices(&self) -> HashMap<i64, PathBuf> {
        HashMap::new()
    }

    fn speaker_name(&self, src: i64) -> String {
        src.to_string()
    }

    /// the latest feed from the game, if any
    fn feed(&self) -> Option<Feed>;

    /// the mixer's feed is fresh (the game is running the mod)
    fn connected(&self) -> bool;

    // ---- what the game wants (from its feed)
    fn wants_mic(&self) -> bool {
        self.feed().is_some_and(|f| f.mic)
    }

    fn language(&self) -> String {
        self.feed().map(|f| f.lang).filter(|l| !l.is_empty()).unwrap_or_else(|| "en".into())
    }

    fn live_words(&self) -> bool {
        self.feed().map(|f| f.live).unwrap_or(true)
    }

    /// live feeds read so far
    fn updates(&self) -> u64 {
        0
    }

    /// what the command line prints at the start: "reading <save>", "my files for the game go to: <dirs>"
    fn describe(&self) -> Vec<String> {
        Vec::new()
    }
}

/// How a game module is made: (the feed's sink: the mixer; the log; a folder for the game's files instead of the
/// usual ones - --io-dir).
pub type MakeGame = fn(Arc<dyn FeedSink>, Log, Option<PathBuf>) -> Box<dyn Game>;

/// A game the program can make: its names, and how to make its module.
#[derive(Clone, Copy)]
pub struct GameKind {
    pub id: &'static str,
    pub name: &'static str,
    pub needs: &'static str,
    pub make: MakeGame,
}

impl std::fmt::Debug for GameKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GameKind").field("id", &self.id).field("name", &self.name).finish()
    }
}

/// The games, in the picker's order.
pub fn games() -> Vec<GameKind> {
    vec![GameKind { id: teardown::ID, name: teardown::NAME, needs: teardown::NEEDS, make: teardown::make }]
}

/// The game with this id; an unknown id: the first.
pub fn by_id(id: &str) -> GameKind {
    let all = games();
    all.iter().find(|g| g.id == id).copied().unwrap_or(all[0])
}
