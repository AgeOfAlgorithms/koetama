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
        }
    }

    internal static class HelperTests
    {
        public static void Run()
        {
            Console.WriteLine("-- helpers");
            // a feed line, read back by an independent parser
            var f = new Feed { Listen = Listen.PushToTalk, TalkKey = true, Lang = "ja", Volume = 0.756, Room = new string('a', 32), Key = new string('b', 64), Me = 7 };
            f.Speakers.Add(new Speaker { Id = 2, Gain = 0.8123, Azimuth = -179.6, Elevation = 10.4, Muffle = 0.6 });
            f.To.Add(2);
            f.Translations.Add(new LanguagePair("ja", "en"));
            string line = f.ToJson(new List<PendingLine> { new PendingLine { Id = 9, Text = "こんにちは \"you\"" } });
            var doc = System.Text.Json.JsonDocument.Parse(line).RootElement;
            Check(doc.GetProperty("type").GetString() == "feed" && doc.GetProperty("listen").GetString() == "push_to_talk"
                  && doc.GetProperty("talk_key").GetBoolean() && doc.GetProperty("volume").GetDouble() == 0.76, "feed basics: " + line);
            var sp = doc.GetProperty("speakers")[0];
            Check(sp.GetProperty("id").GetInt32() == 2 && sp.GetProperty("gain").GetDouble() == 0.81 && sp.GetProperty("azimuth").GetDouble() == -180
                  && sp.GetProperty("elevation").GetDouble() == 10, "speaker rounded (gain 1/100, angles whole degrees)");
            Check(doc.GetProperty("me").GetInt32() == 7 && doc.GetProperty("to")[0].GetInt32() == 2 && doc.GetProperty("room").GetString().Length == 32,
                "room, key, me, to");
            Check(doc.GetProperty("to_translate")[0].GetProperty("text").GetString() == "こんにちは \"you\"", "to_translate text survives");
            f.Me = 0;
            Check(!System.Text.Json.JsonDocument.Parse(f.ToJson(new List<PendingLine>())).RootElement.TryGetProperty("room", out _),
                "no room sent without a player id");

            Check(KoetamaClient.Utf8Prefix("abc", 400) == "abc", "Utf8Prefix keeps a short line");
            string cut = KoetamaClient.Utf8Prefix(new string('é', 300), 400);
            Check(cut.Length == 200, "Utf8Prefix: 400 bytes of 2-byte characters is 200 of them");
            string emoji = KoetamaClient.Utf8Prefix("a" + string.Concat(Enumerable.Repeat("😀", 200)), 400);
            Check(System.Text.Encoding.UTF8.GetByteCount(emoji) == 397 && !char.IsHighSurrogate(emoji[emoji.Length - 1]), "Utf8Prefix never splits a surrogate pair");

            // player ids
            Check(PlayerIds.Small(1234567890123L) == PlayerIds.Small(1234567890123L), "Small is stable");
            var rnd = new Random(1);
            var big = Enumerable.Range(0, 2000).Select(_ => (long)(rnd.NextDouble() * long.MaxValue) * (rnd.Next(2) == 0 ? 1 : -1)).ToList();
            var a = PlayerIds.Assign(big);
            var shuffled = big.OrderBy(_ => rnd.Next()).ToList();
            var b = PlayerIds.Assign(shuffled);
            Check(a.Count == 2000 && a.Values.Distinct().Count() == 2000 && a.Values.All(v => v >= 1 && v <= 65535), "Assign: 2000 players, all different, 1..65535");
            Check(big.All(x => a[x] == b[x]), "Assign gives the same ids in any order (every PC agrees)");
            int moved = big.Count(x => a[x] != PlayerIds.Small(x));
            Check(moved > 0 && moved < 60, "collisions only move a few (" + moved + " of 2000)");

            // spatial
            void Angles(double x, double y, double z, double az, double el, string what)
            {
                Spatial.Angles(x, y, z, out double a1, out double e1);
                Check(Math.Abs(a1 - az) < 1e-6 && Math.Abs(e1 - el) < 1e-6, what + " -> " + a1.ToString("0.#") + ", " + e1.ToString("0.#"));
            }
            Angles(0, 0, 5, 0, 0, "ahead");
            Angles(3, 0, 0, 90, 0, "right");
            Angles(-3, 0, 0, -90, 0, "left");
            Angles(0, 0, -2, 180, 0, "behind");
            Angles(1, 1, 0, 90, 45, "right and up");
            Angles(0, 0, 0, 0, 0, "on top of the listener");
            Check(Spatial.Gain(0, 25) == 1 && Spatial.Gain(25, 25) == 0 && Math.Abs(Spatial.Gain(12.5, 25) - 0.75) < 1e-9 && Spatial.Gain(30, 25) == 0,
                "gain: 1 close, 0.75 halfway, 0 at the range");
        }
    }
}
