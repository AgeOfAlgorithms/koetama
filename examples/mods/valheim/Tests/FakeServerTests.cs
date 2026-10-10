// The client against a fake Koetama: a TcpListener that speaks the socket transport.
using System;
using System.Collections.Concurrent;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Linq;
using System.Net;
using System.Net.Sockets;
using System.Text;
using System.Text.Json;
using System.Threading;
using static Koetama.Tests.Program;

namespace Koetama.Tests
{
    /// <summary>One accepted connection: the lines the client sent, with when they came.</summary>
    internal sealed class FakeConn
    {
        public TcpClient Tcp;
        public readonly ConcurrentQueue<(double t, string line)> Lines = new ConcurrentQueue<(double, string)>();
        public volatile bool Closed;

        public void Send(string text)
        {
            byte[] b = Encoding.UTF8.GetBytes(text);
            Tcp.GetStream().Write(b, 0, b.Length);
        }

        public void SendBytes(byte[] b) { Tcp.GetStream().Write(b, 0, b.Length); }

        public List<(double t, string line)> All() { return Lines.ToList(); }

        public List<JsonElement> Feeds()
        {
            return All().Select(x => JsonDocument.Parse(x.line).RootElement).Where(e => e.GetProperty("type").GetString() == "feed").ToList();
        }
    }

    internal sealed class FakeKoetama : IDisposable
    {
        public readonly int Port;
        private readonly TcpListener listener;
        private readonly Stopwatch clock;
        public readonly BlockingCollection<FakeConn> Accepted = new BlockingCollection<FakeConn>();
        private volatile bool stop;

        public FakeKoetama(Stopwatch clock, int port = 0)
        {
            this.clock = clock;
            listener = new TcpListener(IPAddress.Loopback, port);
            listener.Start();
            Port = ((IPEndPoint)listener.LocalEndpoint).Port;
            new Thread(AcceptLoop) { IsBackground = true }.Start();
        }

        private void AcceptLoop()
        {
            while (!stop)
            {
                TcpClient c;
                try { c = listener.AcceptTcpClient(); }
                catch (Exception) { return; }
                var conn = new FakeConn { Tcp = c };
                new Thread(() => ReadLoop(conn)) { IsBackground = true }.Start();
                Accepted.Add(conn);
            }
        }

        private void ReadLoop(FakeConn conn)
        {
            try
            {
                var reader = new StreamReader(conn.Tcp.GetStream(), new UTF8Encoding(false));
                string line;
                while ((line = reader.ReadLine()) != null) conn.Lines.Enqueue((clock.Elapsed.TotalSeconds, line));
            }
            catch (Exception) { }
            conn.Closed = true;
        }

        public FakeConn Next(double timeout = 5)
        {
            return Accepted.TryTake(out FakeConn c, TimeSpan.FromSeconds(timeout)) ? c : null;
        }

        public void Dispose()
        {
            stop = true;
            listener.Stop();
        }
    }

    internal static class FakeServerTests
    {
        private static readonly Stopwatch Clock = Stopwatch.StartNew();
        private static double Now => Clock.Elapsed.TotalSeconds;

        /// <summary>Calls client.Update every ~16 ms (a game's frames) until cond is true or the time is up.</summary>
        private static bool Frames(KoetamaClient client, Func<bool> cond, double seconds)
        {
            double end = Now + seconds;
            while (Now < end)
            {
                client.Update();
                if (cond()) return true;
                Thread.Sleep(16);
            }
            client.Update();
            return cond();
        }

        private static int FreePort()
        {
            var l = new TcpListener(IPAddress.Loopback, 0);
            l.Start();
            int p = ((IPEndPoint)l.LocalEndpoint).Port;
            l.Stop();
            return p;
        }

        public static void Run()
        {
            Console.WriteLine("-- the client against a fake Koetama");
            int port = FreePort();
            var events = new List<string>();
            var logs = new List<string>();
            var client = new KoetamaClient(port, "Valheim", "Test") { RetryInterval = 0.2, Log = s => logs.Add(s) };
            Hello hello = null;
            Translation translation = null;
            client.Connected += () => events.Add("connected");
            client.Disconnected += () => events.Add("disconnected");
            client.HelloReceived += h => { hello = h; events.Add("hello"); };
            client.SpeechReceived += s => events.Add("speech " + s.Kind + " " + s.Utt + " " + s.Text);
            client.RoomReceived += r => events.Add("room " + r.Id);
            client.VoiceStateChanged += v => events.Add("voice " + v.State + " [" + string.Join(",", v.Players) + "]");
            client.TalkingChanged += tk => events.Add("talking " + tk.Id + " " + tk.IsTalking + " now " + string.Join(",", client.TalkingNow));
            client.StatusChanged += st => events.Add("status " + st.Speech + "/" + st.Microphone + (st.Ready ? " ready" : "") + (client.Status == st ? " kept" : ""));
            client.TranslationReceived += t => { translation = t; events.Add("translation " + t.Id + " " + t.Text); };
            client.TranslationsStatusReceived += st => events.Add("translations into " + st.Into + " " + st.Translations[0].State
                                                                  + (client.TranslationsStatus == st ? " kept" : ""));
            client.Feed.Listen = Listen.Always;
            client.Start();

            // Koetama starts after the game: the client keeps trying
            Frames(client, () => false, 0.5);
            Check(!client.IsConnected, "no Koetama yet: not connected");
            using (var server = new FakeKoetama(Clock, port))
            {
                // (Windows takes ~2 s to refuse a connection to a closed port, so an attempt from before the listener
                // started may be accepted after the client gave up on it: the live connection is the one that talks)
                var seen = new List<FakeConn>();
                FakeConn conn = null;
                Frames(client, () =>
                {
                    while (server.Accepted.TryTake(out FakeConn c)) seen.Add(c);
                    conn = seen.LastOrDefault(c => !c.Closed && c.Lines.Count > 0);
                    return conn != null;
                }, 5);
                Check(conn != null, "the client connects once Koetama listens");
                if (conn == null) return;
                conn.Send("{\"type\":\"hello\",\"app\":\"Koetama\",\"version\":\"0.4.0\",\"protocol\":2,\"features\":[\"speech\",\"voices\",\"rooms\",\"translate\"]}\n");
                Check(Frames(client, () => hello != null && conn.Feeds().Count > 0, 2), "hello received, a feed sent");
                var first = conn.All();
                Check(first[0].line.Contains("\"type\":\"hello\"") && first[0].line.Contains("\"protocol\":2") && first[0].line.Contains("\"game\":\"Valheim\""),
                    "the mod's hello goes first: " + first[0].line);
                Check(conn.Feeds()[0].GetProperty("listen").GetString() == "always", "the feed says listen always");
                Check(client.IsConnected && hello.Has("translate"), "connected; the hello's features");

                // a change goes at once (the talk key), and the same feed is repeated as a heartbeat
                Frames(client, () => false, 0.3);
                int before = conn.Feeds().Count;
                double pressed = Now;
                client.Feed.Listen = Listen.PushToTalk;
                client.Feed.TalkKey = true;
                Check(Frames(client, () => conn.Feeds().Any(f => f.GetProperty("talk_key").GetBoolean()), 1), "talk key pressed: a feed with talk_key true");
                double came = conn.All().First(x => x.line.Contains("\"talk_key\":true")).t;
                Check(came - pressed < 0.05, "... within " + ((came - pressed) * 1000).ToString("0") + " ms");
                Frames(client, () => false, 1.6);
                int repeats = conn.Feeds().Count - before - 1;
                Check(repeats >= 2 && repeats <= 5, "nothing changed for 1.6 s: the feed repeated " + repeats + " times (every 0.5 s)");

                // many small changes (a speaker moving every frame) are paced to one feed per 50 ms at most
                int b2 = conn.Feeds().Count;
                double t0 = Now;
                Frames(client, () =>
                {
                    client.Feed.Speakers.Clear();
                    client.Feed.Speakers.Add(new Speaker { Id = "2", Gain = 1, Azimuth = (Now - t0) * 200 });
                    return false;
                }, 1.0);
                int moving = conn.Feeds().Count - b2;
                Check(moving >= 8 && moving <= 21, "a speaker turning every frame for 1 s: " + moving + " feeds (at most 20)");

                // Koetama's objects, one line each, some split across packets and mid-character
                long id = client.Translate("こんにちは");
                Check(Frames(client, () => conn.Feeds().Any(f => f.GetProperty("to_translate").GetArrayLength() == 1), 1), "a line to translate is in the feed");
                var tt = conn.Feeds().Last().GetProperty("to_translate")[0];
                Check(tt.GetProperty("id").GetInt64() == id && tt.GetProperty("text").GetString() == "こんにちは", "... with its id and text");
                conn.Send("{\"type\":\"speech\",\"kind\":\"start\",\"utt\":4}\n{\"type\":\"speech\",\"kind\":\"live\",\"utt\":4,\"text\":\"hello\",\"times\":[0.1],\"ago\":1.0}\n");
                conn.Send("{\"type\":\"speech\",\"kind\":\"final\",\"utt\":4,\"text\":\"hello there\"}\nnot json at all\n{\"type\":\"a_new_thing\"}\n");
                conn.Send("{\"type\":\"status\",\"speech\":\"loading\",\"microphone\":\"open\"}\n{\"type\":\"status\",\"speech\":\"ready\",\"microphone\":\"open\"}\n");
                conn.Send("{\"type\":\"room\",\"room\":\"" + new string('1', 32) + "\",\"key\":\"" + new string('2', 64) + "\"}\n{\"type\":\"voice\",\"state\":\"connected\",\"players\":[\"-77\",9]}\n");
                conn.Send("{\"type\":\"talking\",\"id\":\"-77\",\"talking\":true}\n{\"type\":\"talking\",\"id\":9,\"talking\":true}\n{\"type\":\"talking\",\"id\":\"-77\",\"talking\":false}\n");
                byte[] tr = Encoding.UTF8.GetBytes("{\"type\":\"translation\",\"id\":" + id + ",\"text\":\"Hello – 你好\"}\n");
                int cut = Array.IndexOf(tr, (byte)0xE2) + 1; // (inside the dash's three bytes)
                conn.SendBytes(tr.Take(cut).ToArray());
                Thread.Sleep(50);
                conn.SendBytes(tr.Skip(cut).ToArray());
                conn.Send("{\"type\":\"translations_status\",\"into\":\"en\",\"translations\":[{\"from\":\"ja\",\"to\":\"en\",\"state\":\"ready\"}]}\n");
                Check(Frames(client, () => events.Any(e => e.StartsWith("translations")), 2), "all of Koetama's objects arrived");
                var got = events.SkipWhile(e => !e.StartsWith("speech")).ToList();
                var want = new[] { "speech Start 4 ", "speech Live 4 hello", "speech Final 4 hello there",
                                   "status loading/open kept", "status ready/open ready kept", "room " + new string('1', 32),
                                   "voice connected [-77,9]", "talking -77 True now -77", "talking 9 True now -77,9", "talking -77 False now 9",
                                   "translation " + id + " Hello – 你好", "translations into en ready kept" };
                Check(got.SequenceEqual(want), "events in order, the bad and unknown lines skipped: " + string.Join(" | ", got));
                Check(translation != null && translation.Original == "こんにちは", "the translation carries its original line");
                Check(logs.Any(l => l.Contains("not JSON")), "the bad line was logged");
                Check(Frames(client, () => conn.Feeds().Last().GetProperty("to_translate").GetArrayLength() == 0, 1), "an answered line leaves the feed");

                // a line never answered is dropped after the timeout
                client.TranslateTimeout = 0.4;
                client.Translate("nobody answers this");
                Check(Frames(client, () => conn.Feeds().Last().GetProperty("to_translate").GetArrayLength() == 1, 1), "an unanswered line is sent");
                Check(Frames(client, () => conn.Feeds().Last().GetProperty("to_translate").GetArrayLength() == 0, 1.5), "... and dropped after the timeout");

                // the game hangs: no more feeds (so Koetama does not keep a talk key held)
                Frames(client, () => false, 0.2);
                client.StallTimeout = 0.5;
                Thread.Sleep(600);
                int c1 = conn.Feeds().Count;
                Thread.Sleep(1200);
                Check(conn.Feeds().Count == c1, "no Update for 1.8 s: the feeds stop");
                client.StallTimeout = 3;
                Check(Frames(client, () => conn.Feeds().Count > c1, 1), "... and start again with the next Update");

                // Koetama goes away and comes back (or replaces the connection): the client reconnects with a new hello
                events.Clear();
                conn.Tcp.Close();
                FakeConn conn2 = null;
                Check(Frames(client, () => (conn2 = conn2 ?? (server.Accepted.TryTake(out var c) ? c : null)) != null, 3), "Koetama closed the connection: the client reconnects");
                Check(Frames(client, () => conn2.All().Count >= 2, 2) && conn2.All()[0].line.Contains("\"hello\"") && conn2.Feeds().Count >= 1,
                    "... with its hello, then the feed at once");
                Check(events.Take(2).SequenceEqual(new[] { "disconnected", "connected" }), "Disconnected, then Connected: " + string.Join(",", events));
                Check(client.TalkingNow.Count == 0 && client.Status == null && client.TranslationsStatus == null,
                    "a new connection forgets who was talking, the status and the translations status");

                // the game asks for a new session (a new voice room): Reconnect
                client.Reconnect();
                FakeConn conn3 = null;
                Check(Frames(client, () => (conn3 = conn3 ?? (server.Accepted.TryTake(out var c) ? c : null)) != null, 3), "Reconnect opens a new connection");
                Check(Frames(client, () => conn2.Closed, 2), "... and closes the old one");
            }
            client.Dispose();
        }
    }
}
