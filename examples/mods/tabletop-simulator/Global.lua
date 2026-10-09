--[[ Koetama Table Talk: Tabletop Simulator's Global script, linked to Koetama by HTTP (PROTOCOL.md "Transport: HTTP").

What it does (on the HOST's PC only: TTS runs every script on the host, and only the host can make web requests):
  - the host's speech as text: each finished line is printed into chat as the host ("Name: what they said");
  - push to talk on a scripting button (numpad 1 by default) held by the host, or "!talk always";
  - chat translation: "!translate es en" (up to two pairs: "!translate ja en ko en", "!translate off"); every chat
    line is sent to the host's Koetama, and its translation is printed under it in another colour.

No proximity voice: a TTS table has no distances worth mixing by (everyone sits at one table), TTS already has its own
global voice chat, and the voice room needs every player's Koetama to get the room and their player ids - which
TTS cannot give them, since no script runs on their PCs.

Chat commands (the host's only; hidden from chat):
  !talk ptt | always | off     how the microphone listens (default: push to talk on the scripting button)
  !lang en                     the language the host speaks (Koetama's codes, or "auto")
  !translate es en [ja en]     translations into the host's language; "!translate off"
  !koetama                     the link's state

Install: in Koetama "Add game mod..." -> examples/profiles/tabletop-simulator-koetama.json, pick it; in TTS:
Modding > Scripting > Global, paste this file, Save & Play. Lua here is MoonSharp (Lua 5.2-ish): no goto, no utf8
library, no integer type.
]]

local PORT = 47160                 -- the profile's
local URL = "http://127.0.0.1:" .. PORT .. "/"
local TALK_BUTTON = 1              -- scripting button 1 = numpad 1
local WAIT = 1                     -- long poll: the answer waits up to 1 s for an object
local LINE_TTL = 10                -- seconds a chat line waits for its translation
local MAX_LINES = 16               -- lines in one feed (the protocol's limit)
local MAX_BYTES = 400              -- bytes of one line (the protocol's limit)
local TRANSLATION_COLOR = {0.65, 0.85, 1.0}
local INFO_COLOR = {0.6, 0.6, 0.6}

local kt = {
    session = 0,          -- made at load: a new session for Koetama (its numbers start over)
    ack = 0,              -- the last object number we have
    polling = false,      -- the long poll is in flight
    urgent = false,       -- an urgent (wait 0) feed is in flight
    urgent_again = false, -- something changed while it was: send another when it lands
    online = nil,         -- nil: not asked yet; true/false: the last request's outcome
    features = {},
    listen = "push_to_talk",
    talk_key = false,
    lang = "en",
    translations = {},    -- {{from=, to=}, ...}
    states = {},          -- "from>to" -> state ("ready", "downloading", ...)
    lines = {},           -- id -> {text=, name=, at=, retried=}: waiting for their translation
    retry = {},           -- lines answered "" while a model was not ready: sent again once ready
    next_id = 1,
    last_progress = {},   -- "from>to" -> the last download percentage shown
}

-- ------------------------------------------------------------------ small helpers

local function now()
    return Time.time
end

local function host()
    for _, list in ipairs({Player.getPlayers(), Player.getSpectators and Player.getSpectators() or {}}) do
        for _, p in ipairs(list) do
            if p.host then return p end
        end
    end
    return nil
end

local function tell_host(msg)
    local h = host()
    if h then h.print("[Koetama] " .. msg, INFO_COLOR) else print("[Koetama] " .. msg) end
end

local function color_of(p)
    local ok, c = pcall(function() return Color.fromString(p.color) end)
    if ok and c then return c end
    return {1, 1, 1}
end

-- A whole number as JSON (MoonSharp has only doubles; "%d" of a big one is not safe everywhere).
local function int(n)
    return string.format("%.0f", n)
end

local function jstr(s)
    s = s:gsub('[%c"\\]', function(c)
        if c == '"' then return '\\"' end
        if c == "\\" then return "\\\\" end
        if c == "\n" then return "\\n" end
        if c == "\r" then return "\\r" end
        if c == "\t" then return "\\t" end
        return string.format("\\u%04x", c:byte())
    end)
    return '"' .. s .. '"'
end

-- At most n bytes of UTF-8, never cutting a character.
local function cut_utf8(s, n)
    if #s <= n then return s end
    local i = n
    while i > 0 do
        local b = s:byte(i + 1)
        if not b or b < 0x80 or b >= 0xC0 then break end -- (the next byte starts a character)
        i = i - 1
    end
    return s:sub(1, i)
end

-- The feed, written by hand: TTS's JSON.encode cannot say "empty list" vs "empty object", and Koetama wants whole
-- numbers for session/ack/id ("3.0" is refused).
local function feed_json(wait)
    local parts = {
        '"type":"feed"',
        '"session":' .. int(kt.session),
        '"ack":' .. int(kt.ack),
        '"wait":' .. (wait > 0 and int(wait) or "0"),
        '"listen":' .. jstr(kt.listen),
        '"talk_key":' .. tostring(kt.talk_key),
        '"lang":' .. jstr(kt.lang),
        '"live":false',
    }
    local tr = {}
    for _, t in ipairs(kt.translations) do
        tr[#tr + 1] = '{"from":' .. jstr(t.from) .. ',"to":' .. jstr(t.to) .. '}'
    end
    parts[#parts + 1] = '"translations":[' .. table.concat(tr, ",") .. ']'
    local lines, ids = {}, {}
    for id, _ in pairs(kt.lines) do ids[#ids + 1] = id end
    table.sort(ids)
    for _, id in ipairs(ids) do
        if #lines >= MAX_LINES then break end
        lines[#lines + 1] = '{"id":' .. int(id) .. ',"text":' .. jstr(kt.lines[id].text) .. '}'
    end
    parts[#parts + 1] = '"to_translate":[' .. table.concat(lines, ",") .. ']'
    return "{" .. table.concat(parts, ",") .. "}"
end

-- ------------------------------------------------------------------ Koetama's objects

local function all_ready()
    for _, t in ipairs(kt.translations) do
        if kt.states[t.from .. ">" .. t.to] ~= "ready" then return false end
    end
    return true
end

local send_urgent -- (below)

local function queue_line(text, name)
    local id = kt.next_id
    kt.next_id = kt.next_id + 1
    kt.lines[id] = {text = cut_utf8(text, MAX_BYTES), name = name, at = now(), retried = false}
    return id
end

local function on_object(o)
    local t = o.type
    if t == "hello" then
        kt.features = {}
        for _, f in ipairs(o.features or {}) do kt.features[f] = true end
        tell_host("linked (Koetama " .. tostring(o.version) .. ", protocol " .. tostring(o.protocol) .. ")")
    elseif t == "speech" then
        if o.kind == "final" and o.text and o.text ~= "" then
            local h = host()
            local name = h and h.steam_name or "Host"
            printToAll(name .. ": " .. o.text, h and color_of(h) or {1, 1, 1})
            if kt.on_said then kt.on_said(o.text) end -- (tests)
        end
    elseif t == "translation" then
        local line = kt.lines[o.id]
        kt.lines[o.id] = nil
        if not line then return end
        if o.text ~= "" then
            printToAll("    > " .. o.text, TRANSLATION_COLOR)
            if kt.on_translated then kt.on_translated(line.text, o.text) end -- (tests)
        elseif not all_ready() and not line.retried then
            -- ("" also means "the models are not ready": ask again once they are, under a new id)
            line.retried = true
            kt.retry[#kt.retry + 1] = line
        end
    elseif t == "translations_status" then
        for _, s in ipairs(o.translations or {}) do
            local key = s.from .. ">" .. s.to
            local was = kt.states[key]
            kt.states[key] = s.state
            if s.state == "downloading" then
                local pct = math.floor((s.progress or 0) * 100)
                if not kt.last_progress[key] or pct >= kt.last_progress[key] + 25 then
                    kt.last_progress[key] = pct
                    tell_host("translation " .. s.from .. " -> " .. s.to .. ": downloading " .. pct .. "%")
                end
            elseif s.state ~= was then
                tell_host("translation " .. s.from .. " -> " .. s.to .. ": " .. s.state)
            end
        end
        if all_ready() and #kt.retry > 0 then
            for _, line in ipairs(kt.retry) do
                local id = queue_line(line.text, line.name)
                kt.lines[id].retried = true
            end
            kt.retry = {}
            send_urgent()
        end
    end
    -- room / voice: this mod does not use voices (no "voices" in the profile), so they never come.
end

-- ------------------------------------------------------------------ the link

local poll -- (below)

local function prune()
    local t = now()
    for id, line in pairs(kt.lines) do
        if t - line.at > LINE_TTL then kt.lines[id] = nil end
    end
end

-- One answer: {"objects":[...],"last":n}. Objects are numbered last-#objects+1 .. last; skip what we have (two
-- requests in flight can both carry an object).
local function on_answer(req)
    if req.is_error or (req.response_code and req.response_code ~= 200) then
        if req.response_code == 400 then
            tell_host("Koetama refused the feed: " .. tostring(req.text))
        elseif kt.online ~= false then
            tell_host("Koetama is not answering on port " .. PORT .. " (start it and pick Tabletop Simulator)")
        end
        kt.online = false
        return false
    end
    kt.online = true
    local ok, a = pcall(JSON.decode, req.text)
    if not ok or type(a) ~= "table" or type(a.last) ~= "number" then return true end
    local objects = a.objects or {}
    local first = a.last - #objects + 1
    for i, o in ipairs(objects) do
        local n = first + i - 1
        if n > kt.ack then
            kt.ack = n
            on_object(o)
        end
    end
    if a.last > kt.ack then kt.ack = a.last end
    return true
end

local function post(wait, done)
    prune()
    WebRequest.custom(URL, "POST", true, feed_json(wait), {["Content-Type"] = "application/json"}, function(req)
        done(on_answer(req))
    end)
end

-- The long poll: one request at a time, the next as soon as an answer lands (so a feed goes at least once a second
-- and each object arrives the moment it exists). Offline: try again every 2 s.
poll = function()
    if kt.polling then return end
    kt.polling = true
    post(WAIT, function(ok)
        kt.polling = false
        if ok then poll() else Wait.time(poll, 2) end
    end)
end

-- Something changed now (the talk key, a chat line to translate): a second, wait-0 request, so it does not sit
-- behind the long poll for up to a second.
send_urgent = function()
    if kt.online == false then return end
    if kt.urgent then kt.urgent_again = true return end
    kt.urgent = true
    post(0, function()
        kt.urgent = false
        if kt.urgent_again then
            kt.urgent_again = false
            send_urgent()
        end
    end)
end

-- ------------------------------------------------------------------ TTS events

function onLoad(_)
    -- a session per load (a game's level): seconds since 1970 x 1000 + a random part, well inside 2^53
    math.randomseed(os.time())
    kt.session = os.time() * 1000 + math.random(0, 999)
    kt.ack = 0
    poll()
end

local function set_talk(on)
    if kt.talk_key ~= on then
        kt.talk_key = on
        send_urgent()
    end
end

function onScriptingButtonDown(index, color)
    local h = host()
    if index == TALK_BUTTON and h and h.color == color and kt.listen == "push_to_talk" then set_talk(true) end
end

function onScriptingButtonUp(index, color)
    local h = host()
    if index == TALK_BUTTON and h and h.color == color then set_talk(false) end
end

local LISTEN = {ptt = "push_to_talk", always = "always", off = "off"}

local function command(words)
    local cmd = words[1]
    if cmd == "!talk" then
        local l = LISTEN[words[2] or ""]
        if not l then tell_host("!talk ptt | always | off") return end
        kt.listen, kt.talk_key = l, false
        tell_host("microphone: " .. l .. (l == "push_to_talk" and " (hold scripting button " .. TALK_BUTTON .. ")" or ""))
    elseif cmd == "!lang" and words[2] then
        kt.lang = words[2]
        tell_host("you speak: " .. kt.lang)
    elseif cmd == "!translate" then
        kt.translations, kt.states, kt.last_progress = {}, {}, {}
        if words[2] ~= "off" then
            local i = 2
            while words[i] and words[i + 1] and #kt.translations < 2 do
                kt.translations[#kt.translations + 1] = {from = words[i], to = words[i + 1]}
                i = i + 2
            end
        end
        if #kt.translations == 0 then tell_host("translation off") end
        if not kt.features["translate"] and kt.online then tell_host("this Koetama profile does not translate") end
    elseif cmd == "!koetama" then
        local tr = {}
        for _, t in ipairs(kt.translations) do
            tr[#tr + 1] = t.from .. "->" .. t.to .. " " .. tostring(kt.states[t.from .. ">" .. t.to] or "?")
        end
        tell_host((kt.online and "linked" or "not linked") .. ", mic " .. kt.listen .. ", lang " .. kt.lang ..
            ", translate: " .. (#tr > 0 and table.concat(tr, ", ") or "off") .. ", last object " .. int(kt.ack))
    else
        return false
    end
    send_urgent()
    return true
end

function onChat(message, player)
    if message:sub(1, 1) == "!" then
        local words = {}
        for w in message:gmatch("%S+") do words[#words + 1] = w:lower() end
        local h = host()
        if h and player.color == h.color and command(words) then return false end
    end
    if #kt.translations > 0 and kt.online then
        queue_line(message, player.steam_name)
        send_urgent()
    end
    return true
end

-- (tests reach the state through this; TTS ignores it)
KOETAMA = kt
