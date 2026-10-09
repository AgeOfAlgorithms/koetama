// What Koetama sends the game (PROTOCOL.md "Koetama -> game"), as typed objects.
using System.Collections.Generic;

namespace Koetama
{
    /// <summary>{"type":"hello"}: first on every connection. Features: what Koetama does for this game.</summary>
    public sealed class Hello
    {
        public string App = "";
        public string Version = "";
        public int Protocol;
        public List<string> Features = new List<string>();

        /// <summary>"speech", "voices", "rooms" or "translate" (an unknown name is simply never there).</summary>
        public bool Has(string feature) { return Features.Contains(feature); }
    }

    public enum SpeechKind { Start, Live, Final }

    /// <summary>{"type":"speech"}: the player started talking, the words so far, or the finished line.</summary>
    public sealed class Speech
    {
        public SpeechKind Kind;
        /// <summary>Numbers the line: its start, live and final share it.</summary>
        public long Utt;
        public string Text = "";
        /// <summary>Each word's start, in seconds after the line's audio began (null: not sent).</summary>
        public double[] Times;
        /// <summary>How long ago the line's audio began when Koetama sent this (NaN: not sent).</summary>
        public double Ago = double.NaN;
    }

    /// <summary>{"type":"room"}: a fresh voice room, once per connection.</summary>
    public sealed class Room
    {
        public string Id = "";
        public string Key = "";
    }

    /// <summary>{"type":"voice"}: the voice chat's link: off, connecting, connected or unreachable.</summary>
    public sealed class VoiceState
    {
        public string State = "";
    }

    /// <summary>{"type":"translation"}: the answer to a line given to KoetamaClient.Translate. Text "": nothing to show.</summary>
    public sealed class Translation
    {
        public long Id;
        public string Text = "";
        /// <summary>The line it translates (kept by the client, not sent by Koetama).</summary>
        public string Original = "";
    }

    /// <summary>One translation's state in {"type":"translations_status"}.</summary>
    public sealed class TranslationState
    {
        public string From = "";
        public string To = "";
        /// <summary>ready, downloading, loading, unavailable or error.</summary>
        public string State = "";
        /// <summary>0..1 while downloading.</summary>
        public double Progress;
    }

    public static class Messages
    {
        /// <summary>
        /// One line from Koetama -> Hello, Speech, Room, VoiceState, Translation or List&lt;TranslationState&gt;; null for
        /// a type this client does not know (a later protocol's). Throws FormatException if the line is not JSON.
        /// </summary>
        public static object Parse(string line)
        {
            var o = Json.Parse(line) as Dictionary<string, object>;
            if (o == null) throw new System.FormatException("not a JSON object");
            switch (Json.GetString(o, "type"))
            {
                case "hello":
                    var h = new Hello
                    {
                        App = Json.GetString(o, "app"),
                        Version = Json.GetString(o, "version"),
                        Protocol = (int)Json.GetLong(o, "protocol"),
                    };
                    foreach (object f in Json.GetArray(o, "features"))
                        if (f is string s) h.Features.Add(s);
                    return h;
                case "speech":
                    string kind = Json.GetString(o, "kind");
                    var sp = new Speech
                    {
                        Kind = kind == "start" ? SpeechKind.Start : kind == "live" ? SpeechKind.Live : SpeechKind.Final,
                        Utt = Json.GetLong(o, "utt"),
                        Text = Json.GetString(o, "text"),
                    };
                    if (o.ContainsKey("times") && o.ContainsKey("ago"))
                    {
                        var times = Json.GetArray(o, "times");
                        sp.Times = new double[times.Count];
                        for (int i = 0; i < times.Count; i++) sp.Times[i] = times[i] is double d ? d : 0;
                        sp.Ago = Json.GetNumber(o, "ago");
                    }
                    return sp;
                case "room":
                    return new Room { Id = Json.GetString(o, "room"), Key = Json.GetString(o, "key") };
                case "voice":
                    return new VoiceState { State = Json.GetString(o, "state") };
                case "translation":
                    return new Translation { Id = Json.GetLong(o, "id", -1), Text = Json.GetString(o, "text") };
                case "translations_status":
                    var list = new List<TranslationState>();
                    foreach (object item in Json.GetArray(o, "translations"))
                    {
                        if (!(item is Dictionary<string, object> t)) continue;
                        list.Add(new TranslationState
                        {
                            From = Json.GetString(t, "from"),
                            To = Json.GetString(t, "to"),
                            State = Json.GetString(t, "state"),
                            Progress = Json.GetNumber(t, "progress"),
                        });
                    }
                    return list;
                default:
                    return null;
            }
        }
    }
}
