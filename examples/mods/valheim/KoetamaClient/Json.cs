// A small JSON reader and writer: enough for Koetama's objects, and nothing a Unity game might not have (Mono's
// class library has no System.Text.Json, and a game's own copy of Newtonsoft is not something to rely on).
using System;
using System.Collections.Generic;
using System.Globalization;
using System.Text;

namespace Koetama
{
    /// <summary>
    /// JSON text -> plain .NET values: an object is a Dictionary&lt;string, object&gt;, an array a List&lt;object&gt;,
    /// a number a double, then string, bool and null.
    /// </summary>
    public static class Json
    {
        /// <summary>Reads one JSON value (white space around it is fine). Throws FormatException on anything else.</summary>
        public static object Parse(string text)
        {
            if (text == null) throw new ArgumentNullException(nameof(text));
            var r = new Reader(text);
            r.SkipSpace();
            object v = r.Value(0);
            r.SkipSpace();
            if (r.Pos != text.Length) throw r.Error("text after the value");
            return v;
        }

        /// <summary>A string as a JSON string literal, quotes included.</summary>
        public static string Quote(string s)
        {
            var sb = new StringBuilder(s.Length + 2);
            AppendQuoted(sb, s);
            return sb.ToString();
        }

        internal static void AppendQuoted(StringBuilder sb, string s)
        {
            sb.Append('"');
            foreach (char c in s)
            {
                switch (c)
                {
                    case '"': sb.Append("\\\""); break;
                    case '\\': sb.Append("\\\\"); break;
                    case '\n': sb.Append("\\n"); break;
                    case '\r': sb.Append("\\r"); break;
                    case '\t': sb.Append("\\t"); break;
                    default:
                        // (other control characters, and the two line separators some JSON readers trip on)
                        if (c < 0x20 || c == (char)0x2028 || c == (char)0x2029)
                            sb.Append("\\u").Append(((int)c).ToString("x4", CultureInfo.InvariantCulture));
                        else
                            sb.Append(c);
                        break;
                }
            }
            sb.Append('"');
        }

        // ---- typed reads from a parsed object, forgiving: a missing or wrong-typed field gives the default

        public static string GetString(IDictionary<string, object> o, string key, string def = "")
        {
            return o.TryGetValue(key, out object v) && v is string s ? s : def;
        }

        public static double GetNumber(IDictionary<string, object> o, string key, double def = 0)
        {
            return o.TryGetValue(key, out object v) && v is double d ? d : def;
        }

        public static long GetLong(IDictionary<string, object> o, string key, long def = 0)
        {
            // (2^53 is the most a double holds exactly; ids are at most 15 digits)
            return o.TryGetValue(key, out object v) && v is double d && Math.Abs(d) < 9007199254740992.0 ? (long)d : def;
        }

        public static bool GetBool(IDictionary<string, object> o, string key, bool def = false)
        {
            return o.TryGetValue(key, out object v) && v is bool b ? b : def;
        }

        /// <summary>A player id as the protocol gives it, a string or a whole number, as a string (null: neither).</summary>
        public static string Id(object v)
        {
            if (v is string s) return s;
            if (v is double d && Math.Abs(d) < 9007199254740992.0 && d == Math.Floor(d)) return ((long)d).ToString(CultureInfo.InvariantCulture);
            return null;
        }

        public static List<object> GetArray(IDictionary<string, object> o, string key)
        {
            return o.TryGetValue(key, out object v) && v is List<object> a ? a : new List<object>();
        }

        private sealed class Reader
        {
            private const int MaxDepth = 64;
            private readonly string s;
            public int Pos;

            public Reader(string text) { s = text; }

            public FormatException Error(string what)
            {
                return new FormatException("JSON: " + what + " at " + Pos);
            }

            public void SkipSpace()
            {
                while (Pos < s.Length && (s[Pos] == ' ' || s[Pos] == '\t' || s[Pos] == '\n' || s[Pos] == '\r')) Pos++;
            }

            public object Value(int depth)
            {
                if (depth > MaxDepth) throw Error("nested too deep");
                if (Pos >= s.Length) throw Error("unexpected end");
                char c = s[Pos];
                switch (c)
                {
                    case '{': return Object(depth);
                    case '[': return Array(depth);
                    case '"': return String();
                    case 't': Word("true"); return true;
                    case 'f': Word("false"); return false;
                    case 'n': Word("null"); return null;
                    default:
                        if (c == '-' || (c >= '0' && c <= '9')) return Number();
                        throw Error("unexpected '" + c + "'");
                }
            }

            private void Word(string w)
            {
                if (string.CompareOrdinal(s, Pos, w, 0, w.Length) != 0) throw Error("expected " + w);
                Pos += w.Length;
            }

            private void Expect(char c)
            {
                if (Pos >= s.Length || s[Pos] != c) throw Error("expected '" + c + "'");
                Pos++;
            }

            private Dictionary<string, object> Object(int depth)
            {
                var o = new Dictionary<string, object>();
                Pos++; // {
                SkipSpace();
                if (Pos < s.Length && s[Pos] == '}') { Pos++; return o; }
                while (true)
                {
                    SkipSpace();
                    if (Pos >= s.Length || s[Pos] != '"') throw Error("expected a key");
                    string key = String();
                    SkipSpace();
                    Expect(':');
                    SkipSpace();
                    o[key] = Value(depth + 1); // (a repeated key: the last one wins)
                    SkipSpace();
                    if (Pos < s.Length && s[Pos] == ',') { Pos++; continue; }
                    Expect('}');
                    return o;
                }
            }

            private List<object> Array(int depth)
            {
                var a = new List<object>();
                Pos++; // [
                SkipSpace();
                if (Pos < s.Length && s[Pos] == ']') { Pos++; return a; }
                while (true)
                {
                    SkipSpace();
                    a.Add(Value(depth + 1));
                    SkipSpace();
                    if (Pos < s.Length && s[Pos] == ',') { Pos++; continue; }
                    Expect(']');
                    return a;
                }
            }

            private string String()
            {
                Pos++; // the opening quote
                var sb = new StringBuilder();
                while (true)
                {
                    if (Pos >= s.Length) throw Error("unterminated string");
                    char c = s[Pos++];
                    if (c == '"') return sb.ToString();
                    if (c < 0x20) throw Error("control character in a string");
                    if (c != '\\') { sb.Append(c); continue; }
                    if (Pos >= s.Length) throw Error("unterminated escape");
                    char e = s[Pos++];
                    switch (e)
                    {
                        case '"': sb.Append('"'); break;
                        case '\\': sb.Append('\\'); break;
                        case '/': sb.Append('/'); break;
                        case 'b': sb.Append('\b'); break;
                        case 'f': sb.Append('\f'); break;
                        case 'n': sb.Append('\n'); break;
                        case 'r': sb.Append('\r'); break;
                        case 't': sb.Append('\t'); break;
                        // (a surrogate pair arrives as two \u escapes; appending both rebuilds the character)
                        case 'u': sb.Append(Hex4()); break;
                        default: throw Error("bad escape \\" + e);
                    }
                }
            }

            private char Hex4()
            {
                if (Pos + 4 > s.Length) throw Error("short \\u escape");
                int v = 0;
                for (int i = 0; i < 4; i++)
                {
                    char h = s[Pos++];
                    int d = h >= '0' && h <= '9' ? h - '0' : h >= 'a' && h <= 'f' ? h - 'a' + 10 : h >= 'A' && h <= 'F' ? h - 'A' + 10 : -1;
                    if (d < 0) throw Error("bad \\u escape");
                    v = v * 16 + d;
                }
                return (char)v;
            }

            private double Number()
            {
                int start = Pos;
                if (s[Pos] == '-') Pos++;
                if (Pos >= s.Length || !char.IsDigit(s[Pos])) throw Error("bad number");
                if (s[Pos] == '0') Pos++;
                else while (Pos < s.Length && s[Pos] >= '0' && s[Pos] <= '9') Pos++;
                if (Pos < s.Length && s[Pos] == '.')
                {
                    Pos++;
                    if (Pos >= s.Length || s[Pos] < '0' || s[Pos] > '9') throw Error("bad number");
                    while (Pos < s.Length && s[Pos] >= '0' && s[Pos] <= '9') Pos++;
                }
                if (Pos < s.Length && (s[Pos] == 'e' || s[Pos] == 'E'))
                {
                    Pos++;
                    if (Pos < s.Length && (s[Pos] == '+' || s[Pos] == '-')) Pos++;
                    if (Pos >= s.Length || s[Pos] < '0' || s[Pos] > '9') throw Error("bad number");
                    while (Pos < s.Length && s[Pos] >= '0' && s[Pos] <= '9') Pos++;
                }
                return double.Parse(s.Substring(start, Pos - start), NumberStyles.Float, CultureInfo.InvariantCulture);
            }
        }
    }

    /// <summary>Writes one JSON object or array into a StringBuilder, with no white space. Numbers are invariant culture.</summary>
    public sealed class JsonWriter
    {
        private readonly StringBuilder sb = new StringBuilder(256);
        // (one flag per open object/array: does the next item need a comma before it)
        private readonly Stack<bool> needComma = new Stack<bool>();

        public override string ToString() { return sb.ToString(); }

        private void Separate()
        {
            if (needComma.Count == 0) return;
            if (needComma.Peek()) sb.Append(',');
            needComma.Pop();
            needComma.Push(true);
        }

        private JsonWriter Key(string key)
        {
            Separate();
            Json.AppendQuoted(sb, key);
            sb.Append(':');
            // (the value that follows is part of this item: no comma before it)
            needComma.Pop();
            needComma.Push(false);
            return this;
        }

        private void Done() { needComma.Pop(); needComma.Push(true); }

        public JsonWriter BeginObject() { Separate(); sb.Append('{'); needComma.Push(false); return this; }
        public JsonWriter EndObject() { needComma.Pop(); sb.Append('}'); return this; }
        public JsonWriter BeginArray() { Separate(); sb.Append('['); needComma.Push(false); return this; }
        public JsonWriter EndArray() { needComma.Pop(); sb.Append(']'); return this; }

        public JsonWriter BeginObject(string key) { Key(key); sb.Append('{'); Done(); needComma.Push(false); return this; }
        public JsonWriter BeginArray(string key) { Key(key); sb.Append('['); Done(); needComma.Push(false); return this; }

        public JsonWriter Value(string v) { Separate(); Json.AppendQuoted(sb, v ?? ""); return this; }
        public JsonWriter Value(long v) { Separate(); sb.Append(v.ToString(CultureInfo.InvariantCulture)); return this; }

        /// <summary>A number rounded to `decimals` places ("0.25", "30", never "1E-05", NaN or infinity: 0).</summary>
        public JsonWriter Value(double v, int decimals)
        {
            Separate();
            AppendNumber(v, decimals);
            return this;
        }

        public JsonWriter Prop(string key, string v) { Key(key); Json.AppendQuoted(sb, v ?? ""); Done(); return this; }
        public JsonWriter Prop(string key, long v) { Key(key); sb.Append(v.ToString(CultureInfo.InvariantCulture)); Done(); return this; }
        public JsonWriter Prop(string key, bool v) { Key(key); sb.Append(v ? "true" : "false"); Done(); return this; }
        public JsonWriter Prop(string key, double v, int decimals) { Key(key); AppendNumber(v, decimals); Done(); return this; }

        private void AppendNumber(double v, int decimals)
        {
            if (double.IsNaN(v) || double.IsInfinity(v)) v = 0;
            double r = Math.Round(v, decimals, MidpointRounding.AwayFromZero);
            if (r == 0) r = 0; // (no "-0")
            sb.Append(r.ToString("0." + new string('#', Math.Max(decimals, 1)), CultureInfo.InvariantCulture));
        }
    }
}
