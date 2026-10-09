// Koetama for Valheim: a demo of Koetama's game API (PROTOCOL.md) in a Unity game.
//   - speech to chat: what the player says (push to talk, B) is said in Valheim's chat as them; the words so far show
//     at the bottom of the screen while they talk; a line ending in "!" is shouted;
//   - proximity voice: the game tells Koetama where everyone is (the listener, each player's head) and how far this
//     player's voice carries (whisper / talk / shout, N); Koetama works out loudness, direction and who gets the
//     voice. The game adds only muffle (a raycast: walls, hills). One voice room per world (VoiceRoom.cs); a speaking
//     icon over each talking player's head, and "Speaking" on the HUD while this player's voice goes out;
//   - translation: other players' chat lines are translated, the translation shown under the line.
// Needs Koetama running with the profile examples/profiles/valheim-koetama.json (port 47131).
using System;
using System.Collections.Generic;
using System.Globalization;
using BepInEx;
using BepInEx.Configuration;
using BepInEx.Logging;
using HarmonyLib;
using Koetama;
using UnityEngine;

namespace KoetamaValheim
{
    [BepInPlugin(Guid, "Koetama", Version)]
    public sealed class Plugin : BaseUnityPlugin
    {
        public const string Guid = "ageofalgorithms.koetama.valheim", Version = "0.1.0";

        public enum ListenMode { Off, Always, PushToTalk }
        public enum VoiceMode { Whisper, Talk, Shout }

        /// <summary>A voice is at full loudness within this part of its range (Koetama fades it to 0 at the range).</summary>
        private const float NearPart = 0.3f;

        private static ManualLogSource logger;

        private ConfigEntry<int> port;
        private ConfigEntry<ListenMode> listenMode;
        private ConfigEntry<KeyCode> talkKey, modeKey;
        private ConfigEntry<string> language, region;
        private ConfigEntry<bool> liveWords, shoutOnExclamation;
        private ConfigEntry<float> volume, whisperRange, talkRange, shoutRange;
        private ConfigEntry<string> tr1From, tr1Into, tr2From, tr2Into;

        private KoetamaClient koetama;
        private VoiceRoom voiceRoom;
        private Harmony harmony;
        private bool headless;
        private VoiceMode voiceMode = VoiceMode.Talk;
        private bool wasInWorld;

        // what the player is saying now, shown at the bottom of the screen
        private string liveText = "";
        private float liveUntil;
        private GUIStyle liveStyle;

        // chat entries waiting for their translation, by line id
        private readonly Dictionary<long, string> translating = new Dictionary<long, string>();
        private readonly Dictionary<string, string> translationStates = new Dictionary<string, string>();
        private int obstacleMask;
        private GUIStyle iconStyle;

        public static void LogError(string s) { logger?.LogError(s); }

        private void Awake()
        {
            logger = Logger;
            port = Config.Bind("Koetama", "Port", 47131, "The port in Koetama's profile for this mod (valheim-koetama.json).");
            listenMode = Config.Bind("Speech", "Listen", ListenMode.PushToTalk,
                "The microphone: Off, Always (Koetama finds each line), or PushToTalk (while the talk key is held).");
            talkKey = Config.Bind("Speech", "TalkKey", KeyCode.B, "Push to talk.");
            language = Config.Bind("Speech", "Language", "en", "The language you speak: en, ja, ko, zh, ru, es, de, ... or auto.");
            liveWords = Config.Bind("Speech", "LiveWords", true, "Show your words while you talk (off: less CPU).");
            shoutOnExclamation = Config.Bind("Speech", "ShoutOnExclamation", true, "A spoken line ending in ! is shouted.");
            volume = Config.Bind("Voice", "Volume", 1f, "How loud the other players are, 0..1.");
            modeKey = Config.Bind("Voice", "ModeKey", KeyCode.N, "Switches how far your voice carries: whisper, talk, shout.");
            whisperRange = Config.Bind("Voice", "WhisperRange", 4f, "Metres a whisper carries.");
            talkRange = Config.Bind("Voice", "TalkRange", 25f, "Metres talking carries.");
            shoutRange = Config.Bind("Voice", "ShoutRange", 70f, "Metres a shout carries.");
            region = Config.Bind("Voice", "Region", "",
                "Where the voice room lives (wnam, enam, weur, apac, ...; empty: near the first player). Everyone in a world must use the same.");
            tr1From = Config.Bind("Translation", "Translation1From", "", "Translate other players' chat from this language (ja, ko, es, ...); empty: off.");
            tr1Into = Config.Bind("Translation", "Translation1Into", "en", "... into this one.");
            tr2From = Config.Bind("Translation", "Translation2From", "", "A second translation; empty: off.");
            tr2Into = Config.Bind("Translation", "Translation2Into", "en", "... into this one.");

            obstacleMask = LayerMask.GetMask("Default", "static_solid", "Default_small", "piece", "terrain");
            voiceRoom = new VoiceRoom(s => Logger.LogInfo(s));
            ChatPatches.OtherPlayerLine = OnOtherPlayerLine;
            harmony = new Harmony(Guid);
            harmony.PatchAll(typeof(ChatPatches));

            // (a dedicated server has no player or microphone, but still hands out the voice room)
            headless = SystemInfo.graphicsDeviceType == UnityEngine.Rendering.GraphicsDeviceType.Null;
            if (headless) return;
            koetama = new KoetamaClient(port.Value, "Valheim", "Koetama for Valheim") { Log = s => Logger.LogInfo(s) };
            koetama.Connected += () => Hud("Koetama connected");
            koetama.Disconnected += () => { if (koetama.Hello != null) Hud("Koetama disconnected"); };
            koetama.HelloReceived += h => Logger.LogInfo("Koetama " + h.Version + ", protocol " + h.Protocol + ": " + string.Join(", ", h.Features.ToArray()));
            koetama.SpeechReceived += OnSpeech;
            koetama.RoomReceived += r => voiceRoom.Offer(r.Id, r.Key);
            koetama.VoiceStateChanged += v => Hud(v.State == "id_taken"
                ? "Voice chat: another player's id clashes with yours in this room (no voice until the next world)"
                : "Voice chat: " + v.State + (v.State == "connected" ? " (" + v.Players.Count + " other" + (v.Players.Count == 1 ? "" : "s") + " in the room)" : ""));
            koetama.StatusChanged += OnStatus;
            koetama.TranslationReceived += OnTranslation;
            koetama.TranslationsStatusReceived += OnTranslationsStatus;
            koetama.Start();
        }

        private void OnDestroy()
        {
            koetama?.Dispose();
            harmony?.UnpatchSelf();
        }

        private void Update()
        {
            Player me = Player.m_localPlayer;
            bool inWorld = ZNet.instance != null;
            voiceRoom.Update(inWorld);
            if (koetama == null) return;

            if (wasInWorld && !inWorld && voiceRoom.UsedOwnOffer)
            {
                // (Koetama makes one room per connection: a new connection, so the next world gets a fresh room)
                voiceRoom.DropOffer();
                koetama.Reconnect();
            }
            wasInWorld = inWorld;

            Feed f = koetama.Feed;
            bool playing = inWorld && me != null && !me.IsDead();
            bool typing = (Chat.instance != null && Chat.instance.HasFocus()) || global::Console.IsVisible() || TextInput.IsVisible() ||
                          Minimap.InTextInput() || Menu.IsVisible();
            f.Listen = !playing ? Listen.Off
                : listenMode.Value == ListenMode.Always ? Listen.Always
                : listenMode.Value == ListenMode.PushToTalk ? Listen.PushToTalk : Listen.Off;
            f.TalkKey = playing && !typing && ZInput.GetKey(talkKey.Value, false);
            f.Lang = language.Value.Trim();
            f.Live = liveWords.Value;
            f.Volume = Mathf.Clamp01(volume.Value);
            f.Region = region.Value.Trim();
            f.Translations.Clear();
            if (tr1From.Value.Trim() != "") f.Translations.Add(new LanguagePair(tr1From.Value.Trim(), tr1Into.Value.Trim()));
            if (tr2From.Value.Trim() != "") f.Translations.Add(new LanguagePair(tr2From.Value.Trim(), tr2Into.Value.Trim()));

            if (playing && !typing && ZInput.GetKeyDown(modeKey.Value, false))
            {
                voiceMode = (VoiceMode)(((int)voiceMode + 1) % 3);
                Hud("Your voice: " + voiceMode.ToString().ToLowerInvariant() + " (" + Range(voiceMode) + " m)");
            }
            UpdateVoices(f, playing ? me : null);
            koetama.Update();
        }

        // ---------------------------------------------------------------- proximity voice

        private float Range(VoiceMode m)
        {
            return m == VoiceMode.Whisper ? whisperRange.Value : m == VoiceMode.Shout ? shoutRange.Value : talkRange.Value;
        }

        /// <summary>The room, this player's id, where they hear from and how far their voice carries, and where each
        /// other player is (Koetama does the loudness, the direction and who gets the voice).</summary>
        private void UpdateVoices(Feed f, Player me)
        {
            f.Speakers.Clear();
            f.Room = voiceRoom.Room;
            f.Key = voiceRoom.Key;
            f.Me = null;
            f.Listener = null;
            f.Range = null;
            Camera cam = Utils.GetMainCamera();
            if (me == null || voiceRoom.Room == null || cam == null) return;

            // Ids: every player's 64-bit peer id (ZNet.GetUID: the owner part of their character's ZDOID, and the
            // sender of their RPCs), as a string - the same on every PC.
            f.Me = ZNet.GetUID().ToString(CultureInfo.InvariantCulture);
            f.Name = me.GetPlayerName();
            Vector3 myHead = me.GetHeadPoint();
            Transform c = cam.transform;
            // (Unity's axes are left-handed: giving all three directions says so)
            f.Listener = new Listener { Position = V(myHead), Forward = V(c.forward), Right = V(c.right), Up = V(c.up) };
            float far = Range(voiceMode);
            f.Range = new VoiceRange(far * NearPart, far);

            float reach = Range(VoiceMode.Shout) * 1.1f; // (no one hears anyone beyond a shout: no raycast needed)
            foreach (Player p in Player.GetAllPlayers())
            {
                if (p == me || p == null || p.GetZDOID().IsNone()) continue;
                Vector3 head = p.GetHeadPoint();
                // (a wall, a hill or a house between: muffled)
                bool blocked = Vector3.Distance(myHead, head) < reach && Physics.Linecast(myHead, head, obstacleMask);
                f.Speakers.Add(new Speaker
                {
                    Id = p.GetZDOID().UserID.ToString(CultureInfo.InvariantCulture),
                    Name = p.GetPlayerName(),
                    Position = V(head),
                    Muffle = blocked ? 0.6 : 0,
                });
            }
        }

        private static Vec3 V(Vector3 v) { return new Vec3(v.x, v.y, v.z); }

        // ---------------------------------------------------------------- who is talking

        private void OnStatus(Status st)
        {
            if (st.Ready) Hud("Koetama ready");
            else if (st.Microphone == "none") Hud("Koetama: no microphone found");
            else if (st.Speech == "loading") Hud("Koetama: loading the speech models");
            else if (st.Speech == "error") Hud("Koetama: speech to text failed (see Koetama's window)");
        }

        /// <summary>A speaking icon over each player whose voice is heard now, and "Speaking" while ours goes out.</summary>
        private void DrawTalking()
        {
            if (koetama == null || koetama.TalkingNow.Count == 0) return;
            Camera cam = Utils.GetMainCamera();
            if (cam == null) return;
            if (iconStyle == null)
            {
                iconStyle = new GUIStyle(GUI.skin.label) { alignment = TextAnchor.MiddleCenter, fontSize = 18, fontStyle = FontStyle.Bold };
                iconStyle.normal.textColor = new Color(0.55f, 1f, 0.6f);
            }
            string me = koetama.Feed.Me;
            foreach (Player p in Player.GetAllPlayers())
            {
                if (p == null || p.GetZDOID().IsNone() || p == Player.m_localPlayer) continue;
                if (!koetama.TalkingNow.Contains(p.GetZDOID().UserID.ToString(CultureInfo.InvariantCulture))) continue;
                Vector3 s = cam.WorldToScreenPoint(p.GetHeadPoint() + Vector3.up * 0.7f);
                if (s.z <= 0) continue; // (behind the camera)
                Shadowed(new Rect(s.x - 60, Screen.height - s.y - 14, 120, 28), "((( talking )))", iconStyle);
            }
            if (me != null && koetama.TalkingNow.Contains(me))
                Shadowed(new Rect(20, Screen.height * 0.62f, 200, 28), "((( Speaking", iconStyle);
        }

        private static void Shadowed(Rect r, string text, GUIStyle style)
        {
            Color c = style.normal.textColor;
            style.normal.textColor = Color.black;
            GUI.Label(new Rect(r.x + 2, r.y + 2, r.width, r.height), text, style);
            style.normal.textColor = c;
            GUI.Label(r, text, style);
        }

        // ---------------------------------------------------------------- speech to chat

        private void OnSpeech(Speech s)
        {
            switch (s.Kind)
            {
                case SpeechKind.Start:
                    liveText = "...";
                    liveUntil = Time.time + 10;
                    break;
                case SpeechKind.Live:
                    liveText = s.Text + " ...";
                    liveUntil = Time.time + 10;
                    break;
                case SpeechKind.Final:
                    liveText = "";
                    string text = s.Text.Trim();
                    if (text.Length == 0 || Chat.instance == null || Player.m_localPlayer == null) return;
                    bool shout = shoutOnExclamation.Value && text.EndsWith("!");
                    Chat.instance.SendText(shout ? Talker.Type.Shout : Talker.Type.Normal, text);
                    break;
            }
        }

        private void OnGUI()
        {
            DrawTalking();
            if (liveText.Length == 0 || Time.time > liveUntil) return;
            if (liveStyle == null)
            {
                liveStyle = new GUIStyle(GUI.skin.label) { alignment = TextAnchor.MiddleCenter, fontSize = 22, wordWrap = true };
                liveStyle.normal.textColor = new Color(1f, 0.92f, 0.7f);
            }
            float w = Screen.width * 0.6f;
            // (a shadow, so it reads over snow too)
            Shadowed(new Rect((Screen.width - w) / 2, Screen.height * 0.78f, w, 60), liveText, liveStyle);
        }

        // ---------------------------------------------------------------- translation

        private void OnOtherPlayerLine(string text, string chatEntry)
        {
            if (koetama == null || koetama.Feed.Translations.Count == 0) return;
            if (koetama.Hello != null && !koetama.Hello.Has("translate")) return;
            translating[koetama.Translate(text)] = chatEntry;
            // (a line Koetama never answers is dropped by the client after 10 s; forget the oldest entries here)
            if (translating.Count > 64)
            {
                long oldest = long.MaxValue;
                foreach (long id in translating.Keys) oldest = Math.Min(oldest, id);
                translating.Remove(oldest);
            }
        }

        private void OnTranslation(Translation t)
        {
            if (!translating.TryGetValue(t.Id, out string entry)) return;
            translating.Remove(t.Id);
            if (t.Text.Trim().Length == 0) return; // (nothing to show: not in a source language, or the same)
            ChatPatches.InsertUnder(entry, "<color=#8fd3ff>    " + ChatPatches.Plain(t.Text) + "</color>");
        }

        private void OnTranslationsStatus(List<TranslationState> states)
        {
            foreach (TranslationState s in states)
            {
                string pair = s.From + " > " + s.To;
                if (translationStates.TryGetValue(pair, out string old) && old == s.State) continue;
                translationStates[pair] = s.State;
                Hud("Translation " + pair + ": " + (s.State == "downloading" ? "downloading the models" : s.State));
            }
        }

        private void Hud(string text)
        {
            Logger.LogInfo(text);
            if (MessageHud.instance != null) MessageHud.instance.ShowMessage(MessageHud.MessageType.TopLeft, text);
        }
    }
}
