// The game's state for Koetama (PROTOCOL.md "Game -> Koetama: the feed"). The game sets these fields on its main
// thread; KoetamaClient.Update turns them into a line and sends it when it changed.
using System.Collections.Generic;

namespace Koetama
{
    public enum Listen { Off, Always, PushToTalk }

    /// <summary>Another player this player hears now, and how.</summary>
    public struct Speaker
    {
        /// <summary>Their player id in the session (1..65535).</summary>
        public int Id;
        /// <summary>0..1; 0: not heard.</summary>
        public double Gain;
        /// <summary>Degrees from where the camera looks: 0 ahead, 90 right, ±180 behind.</summary>
        public double Azimuth;
        /// <summary>Degrees up.</summary>
        public double Elevation;
        /// <summary>0 clear .. 1 (behind walls).</summary>
        public double Muffle;
    }

    /// <summary>"Translate from A into B" (language codes: en, ja, es, ...).</summary>
    public struct LanguagePair
    {
        public string From;
        public string To;
        public LanguagePair(string from, string to) { From = from; To = to; }
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

        public readonly List<Speaker> Speakers = new List<Speaker>();

        /// <summary>The session's voice room (32 hex) and key (64 hex); null: no voices.</summary>
        public string Room;
        public string Key;
        /// <summary>This player's id in the session, 1..65535 (0: none).</summary>
        public int Me;
        /// <summary>Who should get this player's voice now.</summary>
        public readonly List<int> To = new List<int>();
        /// <summary>Where the voice room should live ("" = near the first player); the same for every player.</summary>
        public string Region = "";

        /// <summary>Up to two.</summary>
        public readonly List<LanguagePair> Translations = new List<LanguagePair>();

        /// <summary>
        /// The feed line (without the newline). Angles go to whole degrees and gains to 1/100: finer changes are not
        /// worth a new line, and the line only goes out when it changed.
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
            w.BeginArray("speakers");
            foreach (Speaker s in Speakers)
            {
                w.BeginObject().Prop("id", s.Id).Prop("gain", s.Gain, 2).Prop("azimuth", s.Azimuth, 0)
                    .Prop("elevation", s.Elevation, 0).Prop("muffle", s.Muffle, 2).EndObject();
            }
            w.EndArray();
            if (!string.IsNullOrEmpty(Room) && !string.IsNullOrEmpty(Key) && Me > 0)
            {
                w.Prop("room", Room).Prop("key", Key).Prop("me", Me);
                w.BeginArray("to");
                foreach (int id in To) w.Value(id);
                w.EndArray();
                w.Prop("region", Region ?? "");
            }
            w.BeginArray("translations");
            for (int i = 0; i < Translations.Count && i < 2; i++)
                w.BeginObject().Prop("from", Translations[i].From).Prop("to", Translations[i].To).EndObject();
            w.EndArray();
            w.BeginArray("to_translate");
            foreach (PendingLine p in toTranslate)
                w.BeginObject().Prop("id", p.Id).Prop("text", p.Text).EndObject();
            w.EndArray();
            return w.EndObject().ToString();
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
