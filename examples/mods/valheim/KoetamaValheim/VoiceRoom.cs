// One voice room for everyone in the world (PROTOCOL.md "Real voices": Koetama makes a room per connection, the
// game gives ONE to every player of the session).
//
// The rule, shared over ZRoutedRpc as "Koetama_Room" broadcasts:
//   - The server's choice wins. A server running this mod (a player hosting, or a modded dedicated server) keeps the
//     first room it gets - its own Koetama's, or the first a player offers - and everyone takes it.
//   - With no modded server (a vanilla dedicated server, which still forwards the broadcasts), the earliest claim
//     wins: a claim carries the world time when it was made (the server's clock, the same on every PC), ties broken
//     by the room's text. A player who joins later hears the room within a few seconds and makes no claim of their own.
// Everyone repeats the room they hold every few seconds, so a newcomer learns it without anyone asking.
using System;
using System.Text.RegularExpressions;
using UnityEngine;

namespace KoetamaValheim
{
    internal sealed class VoiceRoom
    {
        private const string RpcName = "Koetama_Room";
        /// <summary>How often the room is repeated (seconds).</summary>
        private const float RepeatEvery = 5f;
        /// <summary>A player who just joined waits this long to hear an existing room before claiming their own.</summary>
        private const float ListenFirst = 6f;
        private static readonly Regex RoomHex = new Regex("^[0-9a-f]{32}$"), KeyHex = new Regex("^[0-9a-f]{64}$");

        public string Room { get; private set; }
        public string Key { get; private set; }
        /// <summary>The room came from this PC's Koetama, so it must not be used again in another world.</summary>
        public bool UsedOwnOffer { get; private set; }

        private bool hostChoice;      // the room we hold is the server's choice
        private long claimMs;         // world time of the claim we hold (ms)
        private string offerRoom, offerKey;
        private ZRoutedRpc registeredOn;
        private float joinedAt, nextRepeat;
        private readonly Action<string> log;

        public VoiceRoom(Action<string> log) { this.log = log; }

        /// <summary>Koetama made a room (once per connection).</summary>
        public void Offer(string room, string key)
        {
            if (!RoomHex.IsMatch(room ?? "") || !KeyHex.IsMatch(key ?? "")) return;
            offerRoom = room;
            offerKey = key;
        }

        /// <summary>Forget the offer once it was used in a world (KoetamaClient.Reconnect brings a fresh one).</summary>
        public void DropOffer()
        {
            offerRoom = offerKey = null;
            UsedOwnOffer = false;
        }

        /// <summary>Every frame. inWorld: a world is loaded (ZNet runs).</summary>
        public void Update(bool inWorld)
        {
            ZRoutedRpc rpc = ZRoutedRpc.instance;
            if (!inWorld || rpc == null)
            {
                Room = Key = null;
                registeredOn = null;
                return;
            }
            if (rpc != registeredOn)
            {
                // (a new world session: ZNet makes a new ZRoutedRpc each time)
                rpc.Register<ZPackage>(RpcName, OnRpc);
                registeredOn = rpc;
                Room = Key = null;
                hostChoice = false;
                joinedAt = Time.time;
                nextRepeat = 0;
            }
            bool server = ZNet.instance.IsServer();
            if (Room == null && offerRoom != null && (server || Time.time - joinedAt > ListenFirst))
            {
                Take(offerRoom, offerKey, WorldMs(), server);
                UsedOwnOffer = true;
                log("voice room: using this PC's room " + Room.Substring(0, 8) + "...");
                nextRepeat = 0;
            }
            if (Room != null && Time.time >= nextRepeat)
            {
                nextRepeat = Time.time + RepeatEvery;
                var pkg = new ZPackage();
                pkg.Write(hostChoice || server);
                pkg.Write(claimMs);
                pkg.Write(Room);
                pkg.Write(Key);
                rpc.InvokeRoutedRPC(ZRoutedRpc.Everybody, RpcName, pkg);
            }
        }

        private void OnRpc(long sender, ZPackage pkg)
        {
            bool fromHost;
            long ms;
            string room, key;
            try
            {
                fromHost = pkg.ReadBool();
                ms = pkg.ReadLong();
                room = pkg.ReadString();
                key = pkg.ReadString();
            }
            catch (Exception)
            {
                return;
            }
            if (!RoomHex.IsMatch(room) || !KeyHex.IsMatch(key) || room == Room) return;
            // (only the server can speak for the server)
            ZNetPeer serverPeer = ZNet.instance != null ? ZNet.instance.GetServerPeer() : null;
            fromHost = fromHost && serverPeer != null && sender == serverPeer.m_uid;
            if (ZNet.instance != null && ZNet.instance.IsServer())
            {
                // (the server keeps the first room it gets)
                if (Room != null) return;
                Take(room, key, ms, true);
            }
            else if (Room == null || (fromHost && !hostChoice) ||
                     (fromHost == hostChoice && (ms < claimMs || (ms == claimMs && string.CompareOrdinal(room, Room) < 0))))
            {
                Take(room, key, ms, fromHost);
            }
            else
            {
                // (ours wins: say so soon, so the other side switches)
                nextRepeat = Math.Min(nextRepeat, Time.time + 0.5f);
                return;
            }
            log("voice room: joined " + room.Substring(0, 8) + "..." + (fromHost ? " (the server's)" : ""));
            nextRepeat = Time.time + RepeatEvery;
        }

        private void Take(string room, string key, long ms, bool fromHost)
        {
            Room = room;
            Key = key;
            claimMs = ms;
            hostChoice = fromHost;
        }

        private static long WorldMs()
        {
            return (long)(ZNet.instance.GetTimeSeconds() * 1000);
        }
    }
}
