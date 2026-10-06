//! The game's state as Kotodama sees it (engine/games/base.py): whom the player hears and how, and what the game
//! wants. A game module reads it from the game (Teardown: its savegame.xml) and hands it to the mixer and the runtime.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One voice to play.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Speaker {
    /// which recorded voice (the game's test speakers: 1 whisperer, 2 speaker, 3 yeller)
    pub src: i64,
    pub talk: bool,
    /// 0..1
    pub gain: f64,
    /// degrees from where the camera looks: 0 ahead, 90 right, +-180 behind
    pub az: f64,
    /// degrees up
    pub el: f64,
    /// 0 clear .. 1 behind walls
    pub muffle: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Feed {
    pub seq: i64,
    /// the player's voice volume in the game, 0..1
    pub vol: f64,
    /// the game's session (a new level: a new one)
    pub sid: i64,
    /// the last message number the game has read
    pub ack: i64,
    /// the game asks "are you there": answered with a file
    pub ping: i64,
    /// the player's speech should be heard and written
    pub mic: bool,
    /// the language the player speaks: "en", "ru", ... or "auto"
    pub lang: String,
    /// live words while they talk (false: only the finished line, less CPU)
    pub live: bool,
    pub speakers: BTreeMap<i64, Speaker>,
}

/// Where a game module hands each new feed: the voice mixer (kd_audio::MixerSink), or a test's recorder.
pub trait FeedSink: Send + Sync {
    fn set_feed(&self, feed: Feed);
    /// a feed came at most kd_audio::STALE s ago (the game is running the mod)
    fn fresh(&self) -> bool;
}
