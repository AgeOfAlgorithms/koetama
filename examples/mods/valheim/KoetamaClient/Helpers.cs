// Small pieces most games need to fill the feed: player ids that fit the protocol, and where a voice comes from.
using System;
using System.Collections.Generic;

namespace Koetama
{
    /// <summary>
    /// The feed's player ids are 1..65535, every PC of the session must use the same one for a player, and games
    /// usually have bigger ones (Valheim: a random 64-bit peer id). These map them with no messages between PCs.
    /// </summary>
    public static class PlayerIds
    {
        /// <summary>A 64-bit id -> 1..65535, the same on every PC (FNV-1a over its 8 bytes).</summary>
        public static int Small(long id)
        {
            uint h = 2166136261;
            for (int i = 0; i < 8; i++)
            {
                h ^= (byte)(id >> (8 * i));
                h *= 16777619;
            }
            return (int)(h % 65535) + 1;
        }

        /// <summary>
        /// The ids of everyone in the session -> small ids, with no two the same: in ascending order of the big id,
        /// each takes Small(id), or the next free number if that is taken. Every PC that knows the same players gets
        /// the same answer; a collision (about 1 in 1500 for 10 players) moves only the later of the two.
        /// </summary>
        public static Dictionary<long, int> Assign(IEnumerable<long> ids)
        {
            var sorted = new List<long>(new HashSet<long>(ids));
            sorted.Sort();
            var taken = new HashSet<int>();
            var result = new Dictionary<long, int>();
            foreach (long id in sorted)
            {
                int s = Small(id);
                while (taken.Contains(s)) s = s % 65535 + 1;
                taken.Add(s);
                result[id] = s;
            }
            return result;
        }
    }

    /// <summary>Where another player's voice comes from, for a Speaker.</summary>
    public static class Spatial
    {
        /// <summary>
        /// A direction in the listener's camera space (x right, y up, z ahead: Unity's camera axes) -> azimuth (0 ahead,
        /// 90 right, ±180 behind) and elevation (degrees up), both in degrees. A zero vector: straight ahead.
        /// </summary>
        public static void Angles(double x, double y, double z, out double azimuth, out double elevation)
        {
            const double deg = 180 / Math.PI;
            azimuth = x == 0 && z == 0 ? 0 : Math.Atan2(x, z) * deg;
            elevation = x == 0 && y == 0 && z == 0 ? 0 : Math.Atan2(y, Math.Sqrt(x * x + z * z)) * deg;
        }

        /// <summary>How loud a voice is at this distance when it carries `range`: 1 up close, 1 - (d/range)^2, 0 beyond.</summary>
        public static double Gain(double distance, double range)
        {
            if (range <= 0 || distance >= range) return 0;
            double f = Math.Max(distance, 0) / range;
            return 1 - f * f;
        }
    }
}
