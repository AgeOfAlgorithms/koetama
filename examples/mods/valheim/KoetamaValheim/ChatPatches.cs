// Valheim's chat, seen from the mod: the lines other players write (to translate), and a way to put a translation
// right under its line.
using System;
using System.Collections.Generic;
using System.Reflection;
using HarmonyLib;
using Splatform;

namespace KoetamaValheim
{
    [HarmonyPatch]
    internal static class ChatPatches
    {
        /// <summary>A line another player wrote (or Koetama said for them): its text, and the chat window's entry for it.</summary>
        public static Action<string, string> OtherPlayerLine;

        private static readonly AccessTools.FieldRef<Terminal, List<string>> ChatBuffer =
            AccessTools.FieldRefAccess<Terminal, List<string>>("m_chatBuffer");
        private static readonly MethodInfo UpdateChat = AccessTools.Method(typeof(Terminal), "UpdateChat");

        private sealed class Before
        {
            public string Text;
            public string LastEntry;
        }

        // Every chat line from a player goes through Terminal.AddString(PlatformUserID, text, type, timestamp), from
        // Chat.OnNewChatMessage (Talker's "Say" RPC for talk and whisper, the routed "ChatMessage" RPC for shouts).
        // The prefix keeps the text as it came (the method upper-cases shouts), the postfix finds the entry it added.
        [HarmonyPatch(typeof(Terminal), nameof(Terminal.AddString),
            new[] { typeof(PlatformUserID), typeof(string), typeof(Talker.Type), typeof(bool) })]
        [HarmonyPrefix]
        private static void BeforeAddString(Terminal __instance, PlatformUserID user, string text, Talker.Type type, out Before __state)
        {
            __state = null;
            if (!(__instance is Chat) || type == Talker.Type.Ping || string.IsNullOrEmpty(text)) return;
            if (Game.instance == null || user.Equals(UserInfo.GetLocalUser().UserId)) return; // (our own lines)
            List<string> buffer = ChatBuffer(__instance);
            __state = new Before { Text = text, LastEntry = buffer.Count > 0 ? buffer[buffer.Count - 1] : null };
        }

        [HarmonyPatch(typeof(Terminal), nameof(Terminal.AddString),
            new[] { typeof(PlatformUserID), typeof(string), typeof(Talker.Type), typeof(bool) })]
        [HarmonyPostfix]
        private static void AfterAddString(Terminal __instance, Before __state)
        {
            if (__state == null) return;
            List<string> buffer = ChatBuffer(__instance);
            string last = buffer.Count > 0 ? buffer[buffer.Count - 1] : null;
            if (ReferenceEquals(last, __state.LastEntry)) return; // (nothing added: an unknown sender)
            try
            {
                OtherPlayerLine?.Invoke(__state.Text, last);
            }
            catch (Exception e)
            {
                Plugin.LogError("chat line: " + e);
            }
        }

        /// <summary>Puts a line into the chat window right under `entry` (at the end if it scrolled away).</summary>
        public static void InsertUnder(string entry, string line)
        {
            Chat chat = Chat.instance;
            if (chat == null) return;
            List<string> buffer = ChatBuffer(chat);
            int i = -1;
            for (int k = buffer.Count - 1; k >= 0; k--)
            {
                if (ReferenceEquals(buffer[k], entry)) { i = k; break; }
            }
            if (i < 0)
            {
                chat.AddString(line);
                return;
            }
            buffer.Insert(i + 1, line);
            UpdateChat.Invoke(chat, null);
        }

        /// <summary>Text from elsewhere, made safe for the chat's rich text (as the game does with chat lines).</summary>
        public static string Plain(string text)
        {
            return text.Replace('<', ' ').Replace('>', ' ');
        }
    }
}
