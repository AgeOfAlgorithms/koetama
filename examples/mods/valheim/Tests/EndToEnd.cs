// The client against the REAL Koetama, headless: koetama.exe --cli --game valheim-koetama --mic-wav <three lines said
// by a Windows voice>, with the repo's examples/profiles/valheim-koetama.json in a temporary profiles folder. The
// client (as the Valheim mod uses it, minus Unity) must get the hello, send a feed with listen "always", receive the
// speech lines, join a voice room, and get a Spanish -> English translation with the real models. E2E_FROM=it
// translates Italian instead (another ~35 MB download: a cold start, if Spanish is already on this PC).
using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Linq;
using System.Text;
using System.Text.RegularExpressions;
using System.Threading;
using static Koetama.Tests.Program;

namespace Koetama.Tests
{
    internal static class EndToEnd
    {
        private const int Port = 47131;
        private const int Rate = 16000;
        private static readonly string[] Lines =
        {
            "Hello everyone, welcome to my base.",
            "Watch out, there is a troll behind the house.",
            "Let's go sailing tomorrow morning and find some iron.",
        };
        private static readonly string[] Italian =
        {
            "Dov'è la nave? C'è un troll vicino alla casa.",
            "Ho bisogno di più legna per costruire il ponte.",
            "Andiamo in montagna domani mattina.",
            "Attenti al drago.",
        };
        private static readonly string[] Spanish =
        {
            "¿Dónde está el barco? Hay un troll cerca de la casa.",
            "Necesito más madera para construir el puente.",
            "Vamos a la montaña mañana por la mañana.",
            "Cuidado con el dragón.",
        };

        private static readonly Stopwatch Clock = Stopwatch.StartNew();
        private static double Now => Clock.Elapsed.TotalSeconds;

        public static void Run()
        {
            string root = FindRepo();
            string exe = Environment.GetEnvironmentVariable("KOETAMA_EXE") ?? Path.Combine(root, "app", "target", "release", "koetama.exe");
            Check(File.Exists(exe), "Koetama is built: " + exe);
            if (!File.Exists(exe)) return;
            string tmp = Path.Combine(Path.GetTempPath(), "koetama-valheim-e2e-" + Guid.NewGuid().ToString("N").Substring(0, 8));
            string profiles = Path.Combine(tmp, "games");
            Directory.CreateDirectory(profiles);
            File.Copy(Path.Combine(root, "examples", "profiles", "valheim-koetama.json"), Path.Combine(profiles, "valheim-koetama.json"));

            // the microphone: each line said by a Windows voice, between quiet stretches
            Console.WriteLine("-- making the recording (Windows voices)");
            var audio = new List<short>();
            var spans = new List<(double start, double end)>();
            audio.AddRange(new short[Rate * 3 / 2]);
            for (int i = 0; i < Lines.Length; i++)
            {
                short[] said = Trim(ReadWav(Say(Lines[i], Path.Combine(tmp, "line" + i + ".wav"))));
                spans.Add((audio.Count / (double)Rate, (audio.Count + said.Length) / (double)Rate));
                audio.AddRange(said);
                audio.AddRange(new short[Rate * 5 / 2]);
            }
            audio.AddRange(new short[Rate * 2]);
            string wav = Path.Combine(tmp, "mic.wav");
            WriteWav(wav, audio.ToArray());
            double duration = audio.Count / (double)Rate;
            for (int i = 0; i < Lines.Length; i++)
                Console.WriteLine($"   line {i + 1}: {spans[i].start:0.00} - {spans[i].end:0.00} s  \"{Lines[i]}\"");

            // Koetama, headless
            var psi = new ProcessStartInfo(exe, $"--cli --game valheim-koetama --mic-wav \"{wav}\" --volume 0 --seconds 400")
            {
                UseShellExecute = false,
                RedirectStandardOutput = true,
                RedirectStandardError = true,
                StandardOutputEncoding = Encoding.UTF8,
                CreateNoWindow = true,
            };
            psi.Environment["KOETAMA_PROFILES_DIR"] = profiles;
            var output = new List<(double t, string line)>();
            double micAt = double.NaN;
            var proc = new Process { StartInfo = psi };
            DataReceivedEventHandler onLine = (s, e) =>
            {
                if (e.Data == null) return;
                lock (output)
                {
                    foreach (string part in e.Data.Split('\r'))
                    {
                        if (part.Trim().Length == 0) continue;
                        output.Add((Now, part));
                        // (the recording starts playing when Koetama opens the "microphone": the clock for the latencies)
                        if (part.Contains("playing the recording") && double.IsNaN(micAt)) micAt = Now;
                    }
                }
            };
            proc.OutputDataReceived += onLine;
            proc.ErrorDataReceived += onLine;
            proc.Start();
            proc.BeginOutputReadLine();
            proc.BeginErrorReadLine();
            double started = Now;

            var client = new KoetamaClient(Port, "Valheim", "Koetama e2e test") { RetryInterval = 0.5 };
            var speech = new List<(double t, Speech s)>();
            var rooms = new List<Room>();
            var voice = new List<(double t, string state)>();
            var translations = new List<(double t, Translation tr)>();
            var statuses = new List<(double t, string text)>();
            double connectedAt = double.NaN;
            client.Connected += () => connectedAt = Now;
            client.SpeechReceived += s => speech.Add((Now, s));
            client.RoomReceived += r => rooms.Add(r);
            client.VoiceStateChanged += v => voice.Add((Now, v.State));
            client.TranslationReceived += t => translations.Add((Now, t));
            client.TranslationsStatusReceived += st => statuses.Add((Now, string.Join(", ",
                st.Select(x => $"{x.From}>{x.To} {x.State}" + (x.State == "downloading" ? $" {x.Progress:0.00}" : "")))));
            client.Feed.Listen = Listen.Always;
            client.Feed.Lang = "en";
            client.Feed.Live = true;
            client.Start();

            try
            {
                Console.WriteLine("-- speech");
                Check(Frames(client, () => client.Hello != null, 30), $"connected and got the hello after {connectedAt - started:0.0} s");
                if (client.Hello == null) return;
                Check(client.Hello.Protocol == 2 && client.Hello.Has("speech") && client.Hello.Has("voices") && client.Hello.Has("rooms")
                      && client.Hello.Has("translate"), "hello: protocol 2, features " + string.Join(", ", client.Hello.Features));
                Check(Frames(client, () => !double.IsNaN(micAt), 60), "Koetama opened the microphone (the recording) after the feed said listen always");
                Frames(client, () => speech.Count(x => x.s.Kind == SpeechKind.Final) >= Lines.Length, duration + 30);
                var finals = speech.Where(x => x.s.Kind == SpeechKind.Final).ToList();
                foreach (var x in speech)
                    Console.WriteLine($"   {x.t - micAt,6:0.00} s  {x.s.Kind,-5} utt {x.s.Utt}: {x.s.Text}" +
                                      (x.s.Times != null ? $"  (ago {x.s.Ago:0.00})" : ""));
                Check(finals.Count == Lines.Length, $"one final line per spoken line ({finals.Count} of {Lines.Length})");
                int wrong = 0;
                var latencies = new List<double>();
                for (int i = 0; i < Math.Min(finals.Count, Lines.Length); i++)
                {
                    wrong += Words(Lines[i]).Except(Words(finals[i].s.Text)).Count() + Words(finals[i].s.Text).Except(Words(Lines[i])).Count();
                    double end = micAt + spans[i].end;
                    latencies.Add(finals[i].t - end);
                    // (the same from the line itself: its audio began `ago` before it was sent)
                    double begun = finals[i].t - finals[i].s.Ago - micAt;
                    Console.WriteLine($"   line {i + 1}: final {(finals[i].t - end) * 1000:0} ms after the speech ended;" +
                                      $" Koetama's line audio began at {begun:0.00} s (the speech at {spans[i].start:0.00} s)");
                }
                Check(wrong <= 3, $"the lines are right ({wrong} words differ)");
                if (latencies.Count > 0)
                    Check(latencies.All(l => l > 0 && l < 3), $"speech end -> final: {string.Join(", ", latencies.Select(l => (l * 1000).ToString("0")))} ms " +
                                                             $"(mean {latencies.Average() * 1000:0} ms)");
                var lives = speech.Where(x => x.s.Kind == SpeechKind.Live).ToList();
                Check(lives.Count > 0 && finals.All(f => f.s.Text.Split(' ').Length < 8 || lives.Any(l => l.s.Utt == f.s.Utt && l.t < f.t)),
                    $"live words before the longer lines' finals ({lives.Count} live messages)");
                var starts = speech.Where(x => x.s.Kind == SpeechKind.Start).ToList();
                if (starts.Count == finals.Count && starts.Count > 0)
                    Console.WriteLine("   start messages came " + string.Join(", ", starts.Select((s, i) =>
                        ((s.t - micAt - spans[i].start) * 1000).ToString("0"))) + " ms after each line began");
                Check(speech.Select(x => x.s.Utt).SequenceEqual(speech.Select(x => x.s.Utt).OrderBy(u => u)), "the speech messages come in utterance order");

                Console.WriteLine("-- voice room");
                Check(rooms.Count == 1 && Regex.IsMatch(rooms[0].Id, "^[0-9a-f]{32}$") && Regex.IsMatch(rooms[0].Key, "^[0-9a-f]{64}$"),
                    "one room for the connection (32 + 64 hex)");
                if (rooms.Count > 0)
                {
                    client.Feed.Room = rooms[0].Id;
                    client.Feed.Key = rooms[0].Key;
                    client.Feed.Me = PlayerIds.Small(123456789);
                    double asked = Now;
                    Check(Frames(client, () => voice.Any(v => v.state == "connected"), 20),
                        "the feed names the room: the voice chat connects to the relay (" + string.Join(" -> ", voice.Select(v => $"{v.state} {v.t - asked:0.00} s")) + ")");
                    client.Feed.Room = client.Feed.Key = null;
                }

                string from = Environment.GetEnvironmentVariable("E2E_FROM") == "it" ? "it" : "es";
                string[] foreign = from == "it" ? Italian : Spanish;
                Console.WriteLine($"-- translation ({from} -> en)");
                client.Feed.Listen = Listen.Off;
                client.Feed.Translations.Add(new LanguagePair(from, "en"));
                double asked0 = Now;
                long early = client.Translate(foreign[0]);
                Check(Frames(client, () => statuses.Any(s => s.text.Contains("ready") || s.text.Contains("unavailable") || s.text.Contains("error")), 300),
                    $"translations_status reached a final state after {statuses.LastOrDefault().t - asked0:0.0} s");
                foreach (var s in statuses.Where((s, i) => i == 0 || s.text.Split(' ')[1] != statuses[i - 1].text.Split(' ')[1] || i == statuses.Count - 1))
                    Console.WriteLine($"   {s.t - asked0,6:0.00} s  {s.text}");
                Frames(client, () => translations.Any(t => t.tr.Id == early), 15);
                var e = translations.FirstOrDefault(t => t.tr.Id == early);
                Console.WriteLine(e.tr != null
                    ? $"   the line sent before the models were ready: answered after {e.t - asked0:0.00} s: \"{e.tr.Text}\""
                    : "   the line sent before the models were ready: no answer within the client's 10 s (dropped)");
                var times = new List<double>();
                foreach (string line in foreign.Skip(1))
                {
                    double t0 = Now;
                    long id = client.Translate(line);
                    bool ok = Frames(client, () => translations.Any(t => t.tr.Id == id), 20);
                    var got = translations.FirstOrDefault(t => t.tr.Id == id);
                    if (ok) times.Add(got.t - t0);
                    Check(ok && got.tr.Text.Length > 0 && got.tr.Text != line && got.tr.Original == line,
                        $"\"{line}\" -> \"{got.tr?.Text}\" in {(got.t - t0) * 1000:0} ms");
                }
                if (times.Count > 0) Console.WriteLine($"   reply time: {string.Join(", ", times.Select(x => (x * 1000).ToString("0")))} ms, mean {times.Average() * 1000:0} ms");
            }
            finally
            {
                client.Dispose();
                try { proc.Kill(); } catch (Exception) { }
                proc.WaitForExit(5000);
                Console.WriteLine("-- Koetama said (excerpt)");
                lock (output)
                {
                    // (not the status line Koetama repeats 4 times a second)
                    foreach (var o in output.Where(o => !o.line.Contains(" | translate") &&
                                                        Regex.IsMatch(o.line, "valheim|Valheim|microphone|voice|translat|model|error|Error|room")).Take(40))
                        Console.WriteLine($"   {o.t - started,6:0.00} s  {o.line.Trim()}");
                }
                try { Directory.Delete(tmp, true); } catch (Exception) { }
            }
        }

        /// <summary>Calls Update every ~16 ms (game frames) until cond holds or the time is up.</summary>
        private static bool Frames(KoetamaClient client, Func<bool> cond, double seconds)
        {
            double end = Now + seconds;
            while (Now < end)
            {
                client.Update();
                if (cond()) return true;
                Thread.Sleep(16);
            }
            return cond();
        }

        private static string FindRepo()
        {
            for (string d = AppContext.BaseDirectory; d != null; d = Path.GetDirectoryName(d))
                if (File.Exists(Path.Combine(d, "PROTOCOL.md"))) return d;
            throw new Exception("not inside the Koetama repo");
        }

        private static IEnumerable<string> Words(string s)
        {
            return Regex.Replace(s.ToLowerInvariant(), @"[^\w' ]", "").Split(new[] { ' ' }, StringSplitOptions.RemoveEmptyEntries).Distinct();
        }

        /// <summary>A Windows voice says text into a 16 kHz mono wav (System.Speech through Windows PowerShell; the
        /// text goes in as an environment variable, never into the script).</summary>
        private static string Say(string text, string path)
        {
            const string script = "Add-Type -AssemblyName System.Speech; $s = New-Object System.Speech.Synthesis.SpeechSynthesizer; " +
                "$f = New-Object System.Speech.AudioFormat.SpeechAudioFormatInfo(16000, [System.Speech.AudioFormat.AudioBitsPerSample]::Sixteen, " +
                "[System.Speech.AudioFormat.AudioChannel]::Mono); $s.SetOutputToWaveFile($env:E2E_OUT, $f); $s.Speak($env:E2E_TEXT); $s.Dispose()";
            var psi = new ProcessStartInfo("powershell", "-NoProfile -NonInteractive -Command \"" + script + "\"") { UseShellExecute = false, CreateNoWindow = true };
            psi.Environment["E2E_OUT"] = path;
            psi.Environment["E2E_TEXT"] = text;
            psi.Environment.Remove("PSModulePath");
            using (var p = Process.Start(psi)) p.WaitForExit(60000);
            if (!File.Exists(path)) throw new Exception("the Windows voice made no " + path);
            return path;
        }

        private static short[] ReadWav(string path)
        {
            byte[] b = File.ReadAllBytes(path);
            int pos = 12;
            while (pos + 8 <= b.Length)
            {
                string id = Encoding.ASCII.GetString(b, pos, 4);
                int size = BitConverter.ToInt32(b, pos + 4);
                if (id == "data")
                {
                    size = Math.Min(size, b.Length - pos - 8);
                    var x = new short[size / 2];
                    Buffer.BlockCopy(b, pos + 8, x, 0, size - size % 2);
                    return x;
                }
                pos += 8 + size + (size & 1);
            }
            throw new Exception("no data in " + path);
        }

        /// <summary>The speech only: the quiet the voice adds before and after it cut off.</summary>
        private static short[] Trim(short[] x)
        {
            const int quiet = 300; // (about -40 dBFS)
            int a = Array.FindIndex(x, v => Math.Abs((int)v) > quiet);
            int b = Array.FindLastIndex(x, v => Math.Abs((int)v) > quiet);
            return a < 0 ? x : x.Skip(a).Take(b - a + 1).ToArray();
        }

        private static void WriteWav(string path, short[] x)
        {
            using (var w = new BinaryWriter(File.Create(path)))
            {
                w.Write(Encoding.ASCII.GetBytes("RIFF"));
                w.Write(36 + x.Length * 2);
                w.Write(Encoding.ASCII.GetBytes("WAVEfmt "));
                w.Write(16);
                w.Write((short)1);
                w.Write((short)1);
                w.Write(Rate);
                w.Write(Rate * 2);
                w.Write((short)2);
                w.Write((short)16);
                w.Write(Encoding.ASCII.GetBytes("data"));
                w.Write(x.Length * 2);
                foreach (short v in x) w.Write(v);
            }
        }
    }
}
