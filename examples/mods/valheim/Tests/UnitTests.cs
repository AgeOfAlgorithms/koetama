using System;
using System.Collections.Generic;
using System.Linq;
using static Koetama.Tests.Program;

namespace Koetama.Tests
{
    internal static class JsonTests
    {
        private static bool Throws(string text)
        {
            try { Json.Parse(text); return false; }
            catch (FormatException) { return true; }
        }

        public static void Run()
        {
            Console.WriteLine("-- JSON");
            var o = (Dictionary<string, object>)Json.Parse(
                " {\"type\":\"speech\", \"utt\": 4, \"times\":[0.1, -2.5e2, 0], \"ok\":true, \"no\":false, \"x\":null, \"o\":{\"a\":[]}} ");
            Check(Json.GetString(o, "type") == "speech" && Json.GetLong(o, "utt") == 4, "an object's strings and numbers");
            var times = Json.GetArray(o, "times");
            Check(times.Count == 3 && (double)times[1] == -250.0, "an array of numbers, with an exponent");
            Check(Json.GetBool(o, "ok") && !Json.GetBool(o, "no", true) && o.ContainsKey("x") && o["x"] == null, "true, false, null");
            Check(((Dictionary<string, object>)o["o"])["a"] is List<object> l && l.Count == 0, "nested empty array");
            Check(Json.GetString(o, "missing", "d") == "d" && Json.GetLong(o, "type", 7) == 7, "a missing or wrong-typed field gives the default");

            Check((string)Json.Parse("\"a\\\"b\\\\c\\/d\\n\\t\\u00e9\"") == "a\"b\\c/d\n\té", "string escapes");
            Check((string)Json.Parse("\"\\ud83d\\ude00\"") == "\U0001F600", "a surrogate pair from two \\u escapes");
            Check((string)Json.Parse("\"こんにちは 😀\"") == "こんにちは 😀", "raw UTF-16 text (CJK, emoji)");
            foreach (string bad in new[] { "{\"a\":1,}", "[1 2]", "\"abc", "{\"a\" 1}", "tru", "01", "-", "1.", "\"\\x\"", "{} x", "", "\"a\nb\"", "{1:2}" })
                Check(Throws(bad), "bad JSON throws: " + bad.Replace("\n", "\\n"));
            Check(Throws(new string('[', 100) + new string(']', 100)), "nesting is limited");

            string q = Json.Quote("he said \"hi\"\\\n\u0001\u2028é");
            Check(q == "\"he said \\\"hi\\\"\\\\\\n\\u0001\\u2028é\"", "Quote escapes what it must: " + q);
            Check((string)Json.Parse(q) == "he said \"hi\"\\\n\u0001\u2028é", "Quote round-trips");

            var w = new JsonWriter().BeginObject().Prop("a", 1).Prop("b", "x").BeginArray("c").Value(1).Value(2.5, 1).BeginObject()
                .Prop("d", true).EndObject().EndArray().Prop("e", -0.0001, 2).Prop("f", double.NaN, 2).Prop("g", 1234567.891, 2).EndObject();
            Check(w.ToString() == "{\"a\":1,\"b\":\"x\",\"c\":[1,2.5,{\"d\":true}],\"e\":0,\"f\":0,\"g\":1234567.89}", "JsonWriter: " + w);
            Check(System.Text.Json.JsonDocument.Parse(w.ToString()).RootElement.GetProperty("c").GetArrayLength() == 3,
                "System.Text.Json reads what JsonWriter writes");

            // the messages
            var h = Messages.Parse("{\"type\":\"hello\",\"app\":\"Koetama\",\"version\":\"0.4.0\",\"protocol\":2,\"features\":[\"speech\",\"voices\",\"rooms\",\"translate\",\"later\"]}") as Hello;
            Check(h != null && h.Protocol == 2 && h.Has("translate") && h.Has("rooms") && !h.Has("x") && h.Version == "0.4.0", "hello");
            var s = Messages.Parse("{\"type\":\"speech\",\"kind\":\"final\",\"utt\":4,\"text\":\"hello there\",\"times\":[0.1,0.55],\"ago\":2.4}") as Speech;
            Check(s != null && s.Kind == SpeechKind.Final && s.Utt == 4 && s.Times.Length == 2 && s.Ago == 2.4, "speech final with times");
            var s2 = Messages.Parse("{\"type\":\"speech\",\"kind\":\"start\",\"utt\":5}") as Speech;
            Check(s2 != null && s2.Kind == SpeechKind.Start && s2.Times == null && double.IsNaN(s2.Ago), "speech start, no times");
            var t = Messages.Parse("{\"type\":\"translation\",\"id\":123456789012345,\"text\":\"Hello\"}") as Translation;
            Check(t != null && t.Id == 123456789012345 && t.Text == "Hello", "translation with a 15-digit id");
            var st = Messages.Parse("{\"type\":\"translations_status\",\"translations\":[{\"from\":\"ja\",\"to\":\"en\",\"state\":\"downloading\",\"progress\":0.42}]}") as List<TranslationState>;
            Check(st != null && st.Count == 1 && st[0].State == "downloading" && st[0].Progress == 0.42, "translations_status");
            Check(Messages.Parse("{\"type\":\"something_new\",\"x\":1}") == null, "an unknown type is null (ignored)");
            Check(Messages.Parse("{\"type\":\"room\",\"room\":\"ab\",\"key\":\"cd\"}") is Room r && r.Id == "ab" && r.Key == "cd", "room");

            // protocol 2's newer objects
            var v = Messages.Parse("{\"type\":\"voice\",\"state\":\"connected\",\"players\":[\"76561198000000002\",12,\"ana\",1.5,null]}") as VoiceState;
            Check(v != null && v.State == "connected" && v.Players.SequenceEqual(new[] { "76561198000000002", "12", "ana" }),
                "voice: state and players (string and number ids as strings, others skipped): " + (v == null ? "" : string.Join(",", v.Players)));
            var v0 = Messages.Parse("{\"type\":\"voice\",\"state\":\"id_taken\"}") as VoiceState;
            Check(v0 != null && v0.State == "id_taken" && v0.Players.Count == 0, "voice: id_taken, no players");
            var tk = Messages.Parse("{\"type\":\"talking\",\"id\":\"-4611686018427387904\",\"talking\":true}") as Talking;
            Check(tk != null && tk.Id == "-4611686018427387904" && tk.IsTalking, "talking: a string id (a Valheim peer id), true");
            var tk2 = Messages.Parse("{\"type\":\"talking\",\"id\":7,\"talking\":false}") as Talking;
            Check(tk2 != null && tk2.Id == "7" && !tk2.IsTalking, "talking: a number id, false");
            var stt = Messages.Parse("{\"type\":\"status\",\"speech\":\"ready\",\"microphone\":\"open\"}") as Status;
            Check(stt != null && stt.Ready && stt.Speech == "ready" && stt.Microphone == "open", "status ready + open: Ready");
            var st2 = Messages.Parse("{\"type\":\"status\",\"speech\":\"loading\",\"microphone\":\"open\"}") as Status;
            var st3 = Messages.Parse("{\"type\":\"status\",\"speech\":\"ready\",\"microphone\":\"none\"}") as Status;
            Check(st2 != null && !st2.Ready && st3 != null && !st3.Ready, "status loading, or no microphone: not Ready");
            var tf = Messages.Parse("{\"type\":\"translation\",\"id\":7,\"text\":\"Hello\",\"from\":\"es\",\"to\":\"en\"}") as Translation;
            Check(tf != null && tf.From == "es" && tf.To == "en" && tf.Text == "Hello", "translation with from / to");
            Check(t.From == "" && t.To == "", "translation without from / to: \"\"");
            Check(Json.Id(12.0) == "12" && Json.Id("x") == "x" && Json.Id(1.5) == null && Json.Id(null) == null && Json.Id(true) == null, "Json.Id");
        }
    }

    internal static class HelperTests
    {
        public static void Run()
        {
            Console.WriteLine("-- helpers");
            // a feed line, read back by an independent parser
            var f = new Feed { Listen = Listen.PushToTalk, TalkKey = true, Lang = "ja", Volume = 0.756, Room = new string('a', 32), Key = new string('b', 64),
                               Me = "-4611686018427387904", Name = "Ana" };
            f.Speakers.Add(new Speaker { Id = "1234567890123", Name = "Ben", Position = new Vec3(3.004, 1.7, -8.126), Muffle = 0.6 });
            f.Speakers.Add(new Speaker { Id = "radio", Gain = 0.8123, Azimuth = -179.6, Elevation = 10.4 });
            f.Listener = new Listener { Position = new Vec3(0, 1.7, 0), Forward = new Vec3(0, 0, 1), Right = new Vec3(1, 0, 0), Up = new Vec3(0, 1, 0) };
            f.Range = new VoiceRange(7.5, 25);
            f.Translations.Add(new LanguagePair("ja", "en"));
            string line = f.ToJson(new List<PendingLine> { new PendingLine { Id = 9, Text = "こんにちは \"you\"" } });
            var doc = System.Text.Json.JsonDocument.Parse(line).RootElement;
            Check(doc.GetProperty("type").GetString() == "feed" && doc.GetProperty("listen").GetString() == "push_to_talk"
                  && doc.GetProperty("talk_key").GetBoolean() && doc.GetProperty("volume").GetDouble() == 0.76, "feed basics: " + line);
            Check(doc.GetProperty("me").GetString() == "-4611686018427387904" && doc.GetProperty("name").GetString() == "Ana", "me as a string id, name");
            var sp = doc.GetProperty("speakers")[0];
            Check(sp.GetProperty("id").GetString() == "1234567890123" && sp.GetProperty("name").GetString() == "Ben"
                  && sp.GetProperty("position")[0].GetDouble() == 3 && sp.GetProperty("position")[2].GetDouble() == -8.13
                  && !sp.TryGetProperty("gain", out _) && !sp.TryGetProperty("azimuth", out _) && sp.GetProperty("muffle").GetDouble() == 0.6,
                "a speaker by position: no gain or angles sent, position to 1/100");
            var sp2 = doc.GetProperty("speakers")[1];
            Check(sp2.GetProperty("gain").GetDouble() == 0.81 && sp2.GetProperty("azimuth").GetDouble() == -180 && sp2.GetProperty("elevation").GetDouble() == 10
                  && !sp2.TryGetProperty("position", out _), "a speaker by gain and angles still works (gain 1/100, whole degrees)");
            var li = doc.GetProperty("listener");
            Check(li.GetProperty("position")[1].GetDouble() == 1.7 && li.GetProperty("forward")[2].GetDouble() == 1 && li.GetProperty("right")[0].GetDouble() == 1
                  && li.GetProperty("up")[1].GetDouble() == 1, "listener: position, forward, right, up");
            Check(doc.GetProperty("range")[0].GetDouble() == 7.5 && doc.GetProperty("range")[1].GetDouble() == 25, "range [near, far]");
            Check(doc.GetProperty("room").GetString().Length == 32 && doc.GetProperty("key").GetString().Length == 64 && !doc.TryGetProperty("room_seed", out _),
                "room and key");
            Check(!doc.TryGetProperty("to", out _), "to left out (null): Koetama sends to whoever is in range");
            Check(doc.GetProperty("to_translate")[0].GetProperty("text").GetString() == "こんにちは \"you\"", "to_translate text survives");
            f.To = new List<string>();
            Check(System.Text.Json.JsonDocument.Parse(f.ToJson(new List<PendingLine>())).RootElement.GetProperty("to").GetArrayLength() == 0, "to empty: nobody");
            f.To = new List<string> { "1234567890123" };
            Check(System.Text.Json.JsonDocument.Parse(f.ToJson(new List<PendingLine>())).RootElement.GetProperty("to")[0].GetString() == "1234567890123", "to: string ids");
            f.RoomSeed = "world 42 + a secret";
            var seeded = System.Text.Json.JsonDocument.Parse(f.ToJson(new List<PendingLine>())).RootElement;
            Check(seeded.GetProperty("room_seed").GetString() == "world 42 + a secret" && !seeded.TryGetProperty("room", out _), "room_seed wins over room and key");
            f.Me = null;
            var noMe = System.Text.Json.JsonDocument.Parse(f.ToJson(new List<PendingLine>())).RootElement;
            Check(!noMe.TryGetProperty("room", out _) && !noMe.TryGetProperty("room_seed", out _) && !noMe.TryGetProperty("me", out _),
                "no room sent without a player id");
            var bare = System.Text.Json.JsonDocument.Parse(new Feed().ToJson(new List<PendingLine>())).RootElement;
            Check(!bare.TryGetProperty("listener", out _) && !bare.TryGetProperty("range", out _) && !bare.TryGetProperty("name", out _),
                "a feed without positions sends no listener, range or name");

            Check(KoetamaClient.Utf8Prefix("abc", 400) == "abc", "Utf8Prefix keeps a short line");
            string cut = KoetamaClient.Utf8Prefix(new string('é', 300), 400);
            Check(cut.Length == 200, "Utf8Prefix: 400 bytes of 2-byte characters is 200 of them");
            string emoji = KoetamaClient.Utf8Prefix("a" + string.Concat(Enumerable.Repeat("😀", 200)), 400);
            Check(System.Text.Encoding.UTF8.GetByteCount(emoji) == 397 && !char.IsHighSurrogate(emoji[emoji.Length - 1]), "Utf8Prefix never splits a surrogate pair");
        }
    }
}
