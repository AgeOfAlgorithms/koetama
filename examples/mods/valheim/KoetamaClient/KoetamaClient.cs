// A game's link to Koetama over the socket transport (PROTOCOL.md "Transport: socket"): one JSON object per line over
// TCP 127.0.0.1:<port>, Koetama listening. Pure .NET (netstandard2.0), no engine references: usable from any C# game.
//
//     var koetama = new KoetamaClient(47131, "Valheim", "Koetama for Valheim");
//     koetama.SpeechReceived += s => { if (s.Kind == SpeechKind.Final) Say(s.Text); };
//     koetama.Start();
//     // every frame, on the game's thread:
//     koetama.Feed.Listen = Listen.PushToTalk; koetama.Feed.TalkKey = keyHeld; ...
//     koetama.Update();    // raises the events that arrived, sends the feed if it changed
//
// Threads: one background thread connects (and reconnects: Koetama may start after the game, or restart), reads
// Koetama's lines into a queue and writes the feed. Everything public is for the game's thread only: the events are
// raised from Update, never from the background thread.
using System;
using System.Collections.Concurrent;
using System.Collections.Generic;
using System.Diagnostics;
using System.Net;
using System.Net.Sockets;
using System.Text;
using System.Threading;

namespace Koetama
{
    public sealed class KoetamaClient : IDisposable
    {
        /// <summary>The protocol this client speaks.</summary>
        public const int Protocol = 2;
        /// <summary>A line this long (bytes) from Koetama is skipped (Koetama's own limit for the game's lines).</summary>
        public const int MaxLine = 64 * 1024;
        /// <summary>Koetama's limits for a line to translate.</summary>
        public const int MaxToTranslate = 16, MaxTranslateBytes = 400;

        /// <summary>A changed feed waits this long after the last one (seconds), unless the talk key or the lines to
        /// translate changed: those go at once.</summary>
        public double MinInterval = 0.05;
        /// <summary>The feed is sent again this often when nothing changed (Koetama counts a game gone after 1.5 s).</summary>
        public double Heartbeat = 0.5;
        /// <summary>Without an Update for this long (the game hangs), no more feeds go out, so Koetama sees the game
        /// gone instead of a talk key held forever.</summary>
        public double StallTimeout = 3;
        /// <summary>A line to translate is dropped after this long without its translation.</summary>
        public double TranslateTimeout = 10;
        /// <summary>How long between tries to reach Koetama.</summary>
        public double RetryInterval = 2;

        /// <summary>What the game tells Koetama. Set its fields on the game's thread, then call Update.</summary>
        public readonly Feed Feed = new Feed();

        public event Action Connected;
        public event Action Disconnected;
        public event Action<Hello> HelloReceived;
        public event Action<Speech> SpeechReceived;
        public event Action<Room> RoomReceived;
        public event Action<VoiceState> VoiceStateChanged;
        public event Action<Translation> TranslationReceived;
        public event Action<List<TranslationState>> TranslationsStatusReceived;

        /// <summary>Diagnostics (called on the game's thread, from Update).</summary>
        public Action<string> Log;

        /// <summary>The connection is open (as of the last Update).</summary>
        public bool IsConnected { get; private set; }
        /// <summary>The last hello (null before the first one).</summary>
        public Hello Hello { get; private set; }

        private readonly string host;
        private readonly int port;
        private readonly string helloLine;
        private readonly Stopwatch clock = Stopwatch.StartNew();
        private readonly ConcurrentQueue<object> inbox = new ConcurrentQueue<object>();
        private readonly List<PendingLine> pending = new List<PendingLine>();
        private readonly Dictionary<long, string> originals = new Dictionary<long, string>();
        private long nextLineId = 1;

        // ---- game thread
        private string lastBuilt;
        private double lastQueuedAt = -1e9;
        private bool lastTalkKey;
        private int pendingVersion, lastPendingVersion;   // (counts changes to the lines to translate)

        // ---- shared with the background thread
        private readonly object gate = new object();
        private string outgoing;          // the feed line to send (the newest one)
        private bool outgoingNew;         // it was not sent yet
        private double lastUpdateAt;      // when the game last called Update
        private volatile bool reconnect;
        private Thread thread;
        private volatile bool stopping;

        public KoetamaClient(int port, string game, string mod, string host = "127.0.0.1")
        {
            this.port = port;
            this.host = host;
            helloLine = new JsonWriter().BeginObject().Prop("type", "hello").Prop("protocol", Protocol)
                .Prop("game", game ?? "").Prop("mod", mod ?? "").EndObject().ToString();
        }

        /// <summary>Starts the background thread (it keeps trying to reach Koetama until Dispose).</summary>
        public void Start()
        {
            if (thread != null) return;
            lastUpdateAt = Now;
            thread = new Thread(Run) { IsBackground = true, Name = "Koetama link" };
            thread.Start();
        }

        /// <summary>Closes the connection and opens a new one: Koetama treats it as a new session (a new voice room).</summary>
        public void Reconnect() { reconnect = true; }

        public void Dispose()
        {
            stopping = true;
            thread?.Join(1000);
            thread = null;
        }

        /// <summary>
        /// Asks Koetama to translate a chat line (Feed.Translations says from and into what). The answer comes as
        /// TranslationReceived with the returned id. The line stays in every feed until then, or TranslateTimeout.
        /// </summary>
        public long Translate(string text)
        {
            long id = nextLineId++;
            string t = Utf8Prefix(text ?? "", MaxTranslateBytes);
            pending.Add(new PendingLine { Id = id, Text = t });
            pendingVersion++;
            originals[id] = t;
            return id;
        }

        /// <summary>The game's thread, every frame: raises what arrived, then sends the feed if it changed.</summary>
        public void Update()
        {
            double now = Now;
            lock (gate) lastUpdateAt = now;
            Dispatch();

            // (lines Koetama never answered: dropped, as the protocol asks)
            for (int i = pending.Count - 1; i >= 0; i--)
            {
                if (!double.IsNaN(pending[i].SentAt) && now - pending[i].SentAt > TranslateTimeout)
                {
                    originals.Remove(pending[i].Id);
                    pending.RemoveAt(i);
                    pendingVersion++;
                }
            }

            int n = Math.Min(pending.Count, MaxToTranslate);
            var lines = pending.GetRange(0, n);
            string line = Feed.ToJson(lines);
            bool urgent = Feed.TalkKey != lastTalkKey || pendingVersion != lastPendingVersion;
            if (line != lastBuilt && (urgent || now - lastQueuedAt >= MinInterval))
            {
                lastBuilt = line;
                lastQueuedAt = now;
                lastTalkKey = Feed.TalkKey;
                lastPendingVersion = pendingVersion;
                lock (gate)
                {
                    outgoing = line;
                    outgoingNew = true;
                }
            }
            // (a line's 10 s start once it is on its way to a connected Koetama)
            if (IsConnected && lastPendingVersion == pendingVersion)
                foreach (PendingLine p in lines)
                    if (double.IsNaN(p.SentAt)) p.SentAt = now;
        }

        private double Now { get { return clock.Elapsed.TotalSeconds; } }

        private void Dispatch()
        {
            while (inbox.TryDequeue(out object m))
            {
                try
                {
                    Raise(m);
                }
                catch (Exception e)
                {
                    // (a game's handler failing must not stop the link)
                    Log?.Invoke("Koetama: a handler failed: " + e);
                }
            }
        }

        private void Raise(object m)
        {
            switch (m)
            {
                case LinkEvent ev:
                    IsConnected = ev.Up;
                    if (ev.Up)
                    {
                        Log?.Invoke("Koetama: connected on port " + port);
                        // (a new connection is a new session: the lines waiting get their 10 s again)
                        foreach (PendingLine p in pending) p.SentAt = double.NaN;
                        Connected?.Invoke();
                    }
                    else
                    {
                        Log?.Invoke("Koetama: disconnected" + (ev.Why != null ? " (" + ev.Why + ")" : ""));
                        Disconnected?.Invoke();
                    }
                    break;
                case Hello h:
                    Hello = h;
                    if (h.Protocol > Protocol) Log?.Invoke("Koetama speaks protocol " + h.Protocol + ", this mod " + Protocol);
                    HelloReceived?.Invoke(h);
                    break;
                case Speech s: SpeechReceived?.Invoke(s); break;
                case Room r: RoomReceived?.Invoke(r); break;
                case VoiceState v: VoiceStateChanged?.Invoke(v); break;
                case Translation t:
                    int i = pending.FindIndex(p => p.Id == t.Id);
                    if (i >= 0) { pending.RemoveAt(i); pendingVersion++; }
                    if (!originals.TryGetValue(t.Id, out string original)) return; // (not ours, or already answered)
                    originals.Remove(t.Id);
                    t.Original = original;
                    TranslationReceived?.Invoke(t);
                    break;
                case List<TranslationState> states: TranslationsStatusReceived?.Invoke(states); break;
                case string bad: Log?.Invoke("Koetama: " + bad); break;
            }
        }

        /// <summary>The longest start of s that fits in maxBytes of UTF-8, never splitting a character.</summary>
        public static string Utf8Prefix(string s, int maxBytes)
        {
            if (Encoding.UTF8.GetByteCount(s) <= maxBytes) return s;
            int bytes = 0, i = 0;
            while (i < s.Length)
            {
                int len = char.IsHighSurrogate(s[i]) && i + 1 < s.Length ? 2 : 1;
                int b = Encoding.UTF8.GetByteCount(s.ToCharArray(i, len));
                if (bytes + b > maxBytes) break;
                bytes += b;
                i += len;
            }
            return s.Substring(0, i);
        }

        // ================================================================ the background thread

        private sealed class LinkEvent
        {
            public bool Up;
            public string Why;
        }

        private void Run()
        {
            var buffer = new byte[8192];
            var line = new List<byte>(1024);
            while (!stopping)
            {
                reconnect = false; // (any new connection is a new session)
                Socket sock = Connect();
                if (sock == null)
                {
                    Sleep(RetryInterval);
                    continue;
                }
                inbox.Enqueue(new LinkEvent { Up = true });
                string why = null;
                bool skipping = false;   // (inside a line that is too long)
                double lastSent = -1e9;
                line.Clear();
                try
                {
                    Send(sock, helloLine);
                    lock (gate) outgoingNew = outgoing != null; // (the newest feed goes out first on a new connection)
                    while (!stopping && !reconnect)
                    {
                        // read what came (5 ms at most: the wait also paces the writes)
                        if (sock.Poll(5000, SelectMode.SelectRead))
                        {
                            int n = sock.Receive(buffer);
                            if (n == 0) { why = "Koetama closed the connection"; break; }
                            for (int i = 0; i < n; i++)
                            {
                                byte b = buffer[i];
                                if (b != (byte)'\n')
                                {
                                    if (!skipping && line.Count < MaxLine) line.Add(b);
                                    else skipping = true;
                                    continue;
                                }
                                if (skipping) inbox.Enqueue("skipped a line over " + MaxLine / 1024 + " KB");
                                else OnLine(Encoding.UTF8.GetString(line.ToArray()));
                                line.Clear();
                                skipping = false;
                            }
                        }
                        // write the feed: a new one at once, else again every Heartbeat while the game runs
                        string toSend = null;
                        double now = Now;
                        lock (gate)
                        {
                            bool alive = now - lastUpdateAt < StallTimeout;
                            if (outgoing != null && alive && (outgoingNew || now - lastSent >= Heartbeat))
                            {
                                toSend = outgoing;
                                outgoingNew = false;
                            }
                        }
                        if (toSend != null)
                        {
                            Send(sock, toSend);
                            lastSent = now;
                        }
                    }
                }
                catch (SocketException e) { why = e.Message; }
                catch (ObjectDisposedException) { why = "closed"; }
                try { sock.Close(); } catch (Exception) { }
                if (reconnect) why = "reconnecting for a new session";
                inbox.Enqueue(new LinkEvent { Up = false, Why = why });
                if (!stopping) Sleep(0.2);
            }
        }

        private Socket Connect()
        {
            var sock = new Socket(AddressFamily.InterNetwork, SocketType.Stream, ProtocolType.Tcp) { NoDelay = true, SendTimeout = 1000 };
            try
            {
                IAsyncResult ar = sock.BeginConnect(IPAddress.Parse(host), port, null, null);
                if (ar.AsyncWaitHandle.WaitOne(1000) && sock.Connected)
                {
                    sock.EndConnect(ar);
                    return sock;
                }
            }
            catch (SocketException) { }
            try { sock.Close(); } catch (Exception) { }
            return null;
        }

        private static void Send(Socket sock, string line)
        {
            byte[] bytes = Encoding.UTF8.GetBytes(line + "\n");
            int sent = 0;
            while (sent < bytes.Length) sent += sock.Send(bytes, sent, bytes.Length - sent, SocketFlags.None);
        }

        private void OnLine(string text)
        {
            if (text.Trim().Length == 0) return;
            try
            {
                object m = Messages.Parse(text);
                if (m != null) inbox.Enqueue(m); // (an unknown type: a later protocol's, ignored)
            }
            catch (FormatException e)
            {
                inbox.Enqueue("skipped a line that is not JSON: " + e.Message);
            }
        }

        private void Sleep(double seconds)
        {
            // (in small steps, so Dispose does not wait long)
            double end = Now + seconds;
            while (!stopping && Now < end) Thread.Sleep(50);
        }
    }
}
