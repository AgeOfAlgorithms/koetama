// The game's state for Koetama (PROTOCOL.md "Game -> Koetama: the feed"). The game sets these fields on its main
// thread; KoetamaClient.Update turns them into a line and sends it when it changed.
using System.Collections.Generic;

namespace Koetama
{
    public enum Listen { Off, Always, PushToTalk }

    /// <summary>A point or a direction, in the game's own units and axes.</summary>
    public struct Vec3
    {
        public double X, Y, Z;
        public Vec3(double x, double y, double z) { X = x; Y = y; Z = z; }
    }

    /// <summary>Where this player hears from: their ears (or the camera) and which way they face.</summary>
    public sealed class Listener
    {
        public Vec3 Position, Forward, Right, Up;
    }

    /// <summary>How far a voice reaches: full loudness within Near, nothing beyond Far (the game's units).</summary>
    public struct VoiceRange
    {
        public double Near, Far;
        public VoiceRange(double near, double far) { Near = near; Far = far; }
    }

    /// <summary>
    /// Another player this player can hear. Give a Position (with Feed.Listener: Koetama works out the direction and,
    /// from that player's range, the loudness), or an Azimuth/Elevation and a Gain worked out by the game.
    /// </summary>
    public struct Speaker
    {
        /// <summary>Their player id in the session, as their own game gives it (Feed.Me on their PC).</summary>
        public string Id;
        /// <summary>Shown in Koetama's window (optional).</summary>
        public string Name;
        /// <summary>Where their mouth is.</summary>
        public Vec3? Position;
        /// <summary>0..1; given, it wins over the distance (0: not heard).</summary>
        public double? Gain;
        /// <summary>Degrees from where the camera looks (0 ahead, 90 right, ±180 behind), for a game without positions.</summary>
        public double? Azimuth;
        /// <summary>Degrees up, for a game without positions.</summary>
        public double? Elevation;
        /// <summary>0 clear .. 1 (behind walls).</summary>
        public double Muffle;
    }

    public sealed class Feed
    {
        /// <summary>0..1: how loud the other players' voices are.</summary>
        public double Volume = 1;
        public Listen Listen = Listen.Off;
        /// <summary>Push to talk: true while the talk key is held (a change is sent at once).</summary>
        public bool TalkKey;
        /// <summary>The language the player speaks ("auto": Koetama finds it).</summary>
        public string Lang = "en";
        /// <summary>Send the words so far while the player talks.</summary>
        public bool Live = true;

        /// <summary>This player's id in the session (1 to 64 characters: a Steam id, a peer id); null: none.</summary>
        public string Me;
        /// <summary>This player's name, shown in the other players' Koetama windows.</summary>
        public string Name;

        public readonly List<Speaker> Speakers = new List<Speaker>();
        /// <summary>Where this player hears from (with the speakers' positions); null: not sent.</summary>
        public Listener Listener;
        /// <summary>How far this player's voice reaches now (whisper, talk, shout); null: not sent.</summary>
        public VoiceRange? Range;

        /// <summary>The session's voice room as a string every player has (Koetama makes room and key from it).</summary>
        public string RoomSeed;
        /// <summary>Or the room (32 hex) and key (64 hex) themselves, from a Room offer the game shared.</summary>
        public string Room;
        public string Key;
        /// <summary>Who gets this player's voice; null: left out (with Range, Listener and positions Koetama sends to
        /// the speakers within reach, else nobody); empty: nobody.</summary>
        public List<string> To;
        /// <summary>Where the voice room should live ("" = near the first player); the same for every player.</summary>
        public string Region = "";

        /// <summary>Koetama may translate the chat lines given to KoetamaClient.Translate (false: stop for a while).
        /// What they are translated into is the player's setting in Koetama's window ("Translate chat into"), not the
        /// game's: KoetamaClient.TranslationsStatus says it.</summary>
        public bool Translate = true;

        /// <summary>
        /// The feed line (without the newline). Positions go to 1/100 of a unit, angles to whole degrees and gains to
        /// 1/100: finer changes are not worth a new line, and the line only goes out when it changed.
        /// </summary>
        internal string ToJson(IList<PendingLine> toTranslate)
        {
            var w = new JsonWriter().BeginObject()
                .Prop("type", "feed")
                .Prop("volume", Volume, 2)
                .Prop("listen", Listen == Listen.Always ? "always" : Listen == Listen.PushToTalk ? "push_to_talk" : "off")
                .Prop("talk_key", TalkKey)
                .Prop("lang", Lang ?? "en")
                .Prop("live", Live);
            if (!string.IsNullOrEmpty(Name)) w.Prop("name", Name);
            w.BeginArray("speakers");
            foreach (Speaker s in Speakers)
            {
                w.BeginObject().Prop("id", s.Id ?? "");
                if (!string.IsNullOrEmpty(s.Name)) w.Prop("name", s.Name);
                if (s.Position.HasValue) Vector(w, "position", s.Position.Value);
                if (s.Gain.HasValue) w.Prop("gain", s.Gain.Value, 2);
                if (s.Azimuth.HasValue) w.Prop("azimuth", s.Azimuth.Value, 0);
                if (s.Elevation.HasValue) w.Prop("elevation", s.Elevation.Value, 0);
                w.Prop("muffle", s.Muffle, 2).EndObject();
            }
            w.EndArray();
            if (Listener != null)
            {
                w.BeginObject("listener");
                Vector(w, "position", Listener.Position);
                Vector(w, "forward", Listener.Forward);
                Vector(w, "right", Listener.Right);
                Vector(w, "up", Listener.Up);
                w.EndObject();
            }
            if (Range.HasValue) w.BeginArray("range").Value(Range.Value.Near, 2).Value(Range.Value.Far, 2).EndArray();
            if (!string.IsNullOrEmpty(Me))
            {
                w.Prop("me", Me);
                if (!string.IsNullOrEmpty(RoomSeed)) w.Prop("room_seed", RoomSeed);
                else if (!string.IsNullOrEmpty(Room) && !string.IsNullOrEmpty(Key)) w.Prop("room", Room).Prop("key", Key);
                if (To != null)
                {
                    w.BeginArray("to");
                    foreach (string id in To) w.Value(id);
                    w.EndArray();
                }
                w.Prop("region", Region ?? "");
            }
            if (!Translate) w.Prop("translate", false);
            w.BeginArray("to_translate");
            foreach (PendingLine p in toTranslate)
                w.BeginObject().Prop("id", p.Id).Prop("text", p.Text).EndObject();
            w.EndArray();
            return w.EndObject().ToString();
        }

        private static void Vector(JsonWriter w, string key, Vec3 v)
        {
            w.BeginArray(key).Value(v.X, 2).Value(v.Y, 2).Value(v.Z, 2).EndArray();
        }
    }

    /// <summary>A chat line waiting for its translation.</summary>
    internal sealed class PendingLine
    {
        public long Id;
        public string Text;
        /// <summary>When it first went out in a feed (seconds on the client's clock; NaN: not yet).</summary>
        public double SentAt = double.NaN;
    }
}
