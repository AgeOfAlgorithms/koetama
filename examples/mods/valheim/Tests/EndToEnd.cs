// The client against the REAL Koetama, headless: TWO Koetamas and two clients, as two players' PCs running the Valheim
// mod (minus Unity), over the live relay:
//   - Ana: koetama.exe --cli --game valheim-koetama --mic-wav <three lines said by a Windows voice>, port 47131 (the
//     repo's examples/profiles/valheim-koetama.json);
//   - Ben: the same profile under another id and port 47132, --no-mic.
// Both feeds name the room Ana's Koetama offered (the mod shares it over the world's RPCs: VoiceRoom.cs), with string
// player ids (64-bit peer ids), a listener, positions and a range - no gain, direction or "to". Ana's client must get
// the hello, the status (ready, microphone open), the speech lines and her own talking; Ben's must see Ana in the room
// (voice players) and get talking true / false for each stretch she speaks. Then a Spanish -> English translation with
// the real models. E2E_FROM=it translates Italian instead (another ~35 MB download: a cold start, if Spanish is
// already on this PC).
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
        private const int Port = 47131, PortB = 47132;
        // (Valheim's peer ids: random 64-bit numbers, either sign)
        private const string AnaId = "1838205539120117842", BenId = "-2971620419488523405";
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
            string profile = File.ReadAllText(Path.Combine(root, "examples", "profiles", "valheim-koetama.json"));
            File.WriteAllText(Path.Combine(profiles, "valheim-koetama.json"), profile);
            // (Ben's Koetama: the same mod on a second port, as if on another PC)
            File.WriteAllText(Path.Combine(profiles, "valheim-koetama-b.json"),
                profile.Replace("\"valheim-koetama\"", "\"valheim-koetama-b\"").Replace("47131", PortB.ToString()));

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

            // two Koetamas, headless
            var output = new List<(double t, string line)>();
            double micAt = double.NaN;
            Process Start(string args, string who)
            {
                var psi = new ProcessStartInfo(exe, args + " --volume 0 --seconds 400")
                {
                    UseShellExecute = false,
                    RedirectStandardOutput = true,
                    RedirectStandardError = true,
                    StandardOutputEncoding = Encoding.UTF8,
                    CreateNoWindow = true,
                };
                psi.Environment["KOETAMA_PROFILES_DIR"] = profiles;
                var proc = new Process { StartInfo = psi };
                DataReceivedEventHandler onLine = (s, e) =>
                {
                    if (e.Data == null) return;
                    lock (output)
                    {
                        foreach (string part in e.Data.Split('\r'))
                        {
                            if (part.Trim().Length == 0) continue;
                            output.Add((Now, who + ": " + part));
                            // (the recording starts playing when Koetama opens the "microphone": the clock for the latencies)
                            if (who == "ana" && part.Contains("playing the recording") && double.IsNaN(micAt)) micAt = Now;
                        }
                    }
                };
                proc.OutputDataReceived += onLine;
                proc.ErrorDataReceived += onLine;
                proc.Start();
                proc.BeginOutputReadLine();
                proc.BeginErrorReadLine();
                return proc;
            }
            Process procA = Start($"--cli --game valheim-koetama --mic-wav \"{wav}\"", "ana");
            Process procB = Start("--cli --game valheim-koetama-b --no-mic", "ben");
            double started = Now;

            var client = new KoetamaClient(Port, "Valheim", "Koetama e2e test") { RetryInterval = 0.5 };
            var ben = new KoetamaClient(PortB, "Valheim", "Koetama e2e test (Ben)") { RetryInterval = 0.5 };
            clients = new[] { client, ben };
            var speech = new List<(double t, Speech s)>();
            var rooms = new List<Room>();
            var voice = new List<(double t, VoiceState v)>();
            var benVoice = new List<(double t, VoiceState v)>();
            var talking = new List<(double t, Talking tk)>();
            var benTalking = new List<(double t, Talking tk)>();
            var status = new List<(double t, Status st)>();
            var benStatus = new List<(double t, Status st)>();
            var translations = new List<(double t, Translation tr)>();
            var statuses = new List<(double t, string text)>();
            double connectedAt = double.NaN;
            client.Connected += () => connectedAt = Now;
            client.SpeechReceived += s => speech.Add((Now, s));
            client.VoiceStateChanged += v => voice.Add((Now, v));
            ben.VoiceStateChanged += v => benVoice.Add((Now, v));
            client.TalkingChanged += tk => talking.Add((Now, tk));
            ben.TalkingChanged += tk => benTalking.Add((Now, tk));
            client.StatusChanged += st => status.Add((Now, st));
            ben.StatusChanged += st => benStatus.Add((Now, st));
            client.TranslationReceived += t => translations.Add((Now, t));
            client.TranslationsStatusReceived += st => statuses.Add((Now, "into " + st.Into + ": " + string.Join(", ",
                st.Translations.Select(x => $"{x.From}>{x.To} {x.State}" + (x.State == "downloading" ? $" {x.Progress:0.00}" : "")))));

            // Ana at the origin facing +z, Ben 5 m away (3 right, 4 ahead) facing her: positions and a range, no gains
            var up = new Vec3(0, 1, 0);
            client.Feed.Me = AnaId;
            client.Feed.Name = "Ana";
            client.Feed.Listen = Listen.Always;
            client.Feed.Lang = "en";
            client.Feed.Live = true;
            client.Feed.Listener = new Listener { Position = new Vec3(0, 1.7, 0), Forward = new Vec3(0, 0, 1), Right = new Vec3(1, 0, 0), Up = up };
            client.Feed.Range = new VoiceRange(7.5, 25);
            client.Feed.Speakers.Add(new Speaker { Id = BenId, Name = "Ben", Position = new Vec3(3, 1.7, 4) });
            ben.Feed.Me = BenId;
            ben.Feed.Name = "Ben";
            ben.Feed.Listen = Listen.Off;
            ben.Feed.Listener = new Listener { Position = new Vec3(3, 1.7, 4), Forward = new Vec3(-0.6, 0, -0.8), Right = new Vec3(-0.8, 0, 0.6), Up = up };
            ben.Feed.Range = new VoiceRange(7.5, 25);
            ben.Feed.Speakers.Add(new Speaker { Id = AnaId, Name = "Ana", Position = new Vec3(0, 1.7, 0) });
            // (the room Ana's Koetama offers goes to both, as VoiceRoom.cs shares it; Ben's own offer is not used)
            client.RoomReceived += r =>
            {
                rooms.Add(r);
                client.Feed.Room = ben.Feed.Room = r.Id;
                client.Feed.Key = ben.Feed.Key = r.Key;
            };
            client.Start();
            ben.Start();

            try
            {
                Console.WriteLine("-- speech");
                Check(Frames(() => client.Hello != null && ben.Hello != null, 30), $"connected and got the hello after {connectedAt - started:0.0} s (both)");
                if (client.Hello == null) return;
                Check(client.Hello.Protocol == 2 && client.Hello.Has("speech") && client.Hello.Has("voices") && client.Hello.Has("rooms")
                      && client.Hello.Has("translate"), "hello: protocol 2, features " + string.Join(", ", client.Hello.Features));
                Check(Frames(() => !double.IsNaN(micAt), 60), "Koetama opened the microphone (the recording) after the feed said listen always");
                Check(Frames(() => client.Status != null && client.Status.Ready, 30),
                    "status: " + string.Join(" -> ", status.Select(x => $"{x.st.Speech}/{x.st.Microphone} {x.t - started:0.0} s")) + " (Ready)");
                Frames(() => speech.Count(x => x.s.Kind == SpeechKind.Final) >= Lines.Length, duration + 30);
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

                Console.WriteLine("-- voice room (two Koetamas, the live relay)");
                Frames(() => benTalking.Count(x => !x.tk.IsTalking) >= Lines.Length, 5);
                Check(rooms.Count == 1 && Regex.IsMatch(rooms[0].Id, "^[0-9a-f]{32}$") && Regex.IsMatch(rooms[0].Key, "^[0-9a-f]{64}$"),
                    "one room offered on Ana's connection (32 + 64 hex), named in both feeds");
                Console.WriteLine("   Ana's voice: " + string.Join(" -> ", voice.Select(v => $"{v.v.State} [{string.Join(",", v.v.Players)}] {v.t - started:0.0} s")));
                Console.WriteLine("   Ben's voice: " + string.Join(" -> ", benVoice.Select(v => $"{v.v.State} [{string.Join(",", v.v.Players)}] {v.t - started:0.0} s")));
                Check(Frames(() => voice.Any(v => v.v.State == "connected" && v.v.Players.Contains(BenId)), 15), "Ana's Koetama sees Ben in the room (voice players, his string id)");
                Check(Frames(() => benVoice.Any(v => v.v.State == "connected" && v.v.Players.Contains(AnaId)), 15), "Ben's Koetama sees Ana in the room");
                foreach (var x in benTalking)
                    Console.WriteLine($"   Ben hears {(x.tk.Id == AnaId ? "Ana" : x.tk.Id)} {(x.tk.IsTalking ? "start" : "stop ")} at {x.t - micAt,6:0.00} s");
                var benOn = benTalking.Where(x => x.tk.Id == AnaId && x.tk.IsTalking).ToList();
                var benOff = benTalking.Where(x => x.tk.Id == AnaId && !x.tk.IsTalking).ToList();
                Check(benOn.Count >= 1 && benOff.Count >= 1, $"Ben's game: talking true / false for Ana ({benOn.Count} starts, {benOff.Count} stops for {Lines.Length} lines)");
                if (benOn.Count == Lines.Length)
                    Console.WriteLine("   Ana's voice reached Ben's game " + string.Join(", ", benOn.Select((x, i) => ((x.t - micAt - spans[i].start) * 1000).ToString("0"))) +
                                      " ms after each line began; it stopped " + string.Join(", ", benOff.Take(Lines.Length).Select((x, i) => ((x.t - micAt - spans[i].end) * 1000).ToString("0"))) +
                                      " ms after each line ended");
                var ownOn = talking.Where(x => x.tk.Id == AnaId && x.tk.IsTalking).ToList();
                Check(ownOn.Count >= 1, $"Ana's game: her own talking (id = me) while her voice goes out ({ownOn.Count} starts)");
                Check(!ben.TalkingNow.Contains(AnaId) && !client.TalkingNow.Contains(AnaId), "after the last line nobody is talking (TalkingNow empty)");
                Console.WriteLine("   Ben's status: " + string.Join(" -> ", benStatus.Select(x => $"{x.st.Speech}/{x.st.Microphone}")));

                string from = Environment.GetEnvironmentVariable("E2E_FROM") == "it" ? "it" : "es";
                string[] foreign = from == "it" ? Italian : Spanish;
                // (into English: Koetama's own setting, "Translate chat into" in its window; set it before the run)
                Console.WriteLine($"-- translation ({from} -> en)");
                client.Feed.Listen = Listen.Off;
                double asked0 = Now;
                long early = client.Translate(foreign[0]);
                Check(Frames(() => statuses.Any(s => s.text.Contains("ready") || s.text.Contains("unavailable") || s.text.Contains("error")), 300),
                    $"translations_status reached a final state after {statuses.LastOrDefault().t - asked0:0.0} s");
                foreach (var s in statuses.Where((s, i) => i == 0 || Regex.Replace(s.text, @" [0-9]\.[0-9]+", "") != Regex.Replace(statuses[i - 1].text, @" [0-9]\.[0-9]+", "") || i == statuses.Count - 1))
                    Console.WriteLine($"   {s.t - asked0,6:0.00} s  {s.text}");
                Frames(() => translations.Any(t => t.tr.Id == early), 15);
                var e = translations.FirstOrDefault(t => t.tr.Id == early);
                Console.WriteLine(e.tr != null
                    ? $"   the line sent before the models were ready: answered after {e.t - asked0:0.00} s: \"{e.tr.Text}\""
                    : "   the line sent before the models were ready: no answer within the client's 10 s (dropped)");
                var times = new List<double>();
                foreach (string line in foreign.Skip(1))
                {
                    double t0 = Now;
                    long id = client.Translate(line);
                    bool ok = Frames(() => translations.Any(t => t.tr.Id == id), 20);
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
                ben.Dispose();
                foreach (Process proc in new[] { procA, procB })
                {
                    try { proc.Kill(); } catch (Exception) { }
                    proc.WaitForExit(5000);
                }
                Console.WriteLine("-- Koetama said (excerpt)");
                lock (output)
                {
                    // (not the status line Koetama repeats 4 times a second)
                    foreach (var o in output.Where(o => !o.line.Contains(" | translate") &&
                                                        Regex.IsMatch(o.line, "valheim|Valheim|microphone|voice|translat|model|error|Error|room")).Take(50))
                        Console.WriteLine($"   {o.t - started,6:0.00} s  {o.line.Trim()}");
                }
                try { Directory.Delete(tmp, true); } catch (Exception) { }
            }
        }

        private static KoetamaClient[] clients = new KoetamaClient[0];

        /// <summary>Calls each client's Update every ~16 ms (game frames) until cond holds or the time is up.</summary>
        private static bool Frames(Func<bool> cond, double seconds)
        {
            double end = Now + seconds;
            while (Now < end)
            {
                foreach (KoetamaClient c in clients) c.Update();
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
