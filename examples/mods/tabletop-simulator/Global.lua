--[[ Koetama Table Talk: Tabletop Simulator's Global script, linked to Koetama by HTTP (PROTOCOL.md "Transport: HTTP").

TTS runs every script on the HOST's PC only, and only the host can make web requests: the other players' games tell
their Koetamas nothing. So the host's Koetama works as a hub (PROTOCOL.md "Hub"): the host's feed lists every other
seated player, Koetama answers a join code for each, and this script shows each player their code privately. A player
types it into their own Koetama ("Join a game with a code"), and from then on their microphone, speech to text and
translations are theirs, on their PC, with what they say arriving here tagged with them.

What it does:
  - speech as text: each finished line, the host's or a joined player's, is printed into chat as them, in their colour;
  - push to talk on a scripting button (numpad 1 by default), each player their own, or "!talk always";
  - chat translation, per player: every chat line goes to each player's Koetama (not its writer's), and the
    translation is printed to that player only, under the line. What it is translated into is the player's own
    setting in Koetama's window ("Translate chat into"), not this script's: a player whose Koetama says it is off
    gets no lines;
  - "Name joined with their Koetama" / "... left" for everyone;
  - "!voice on" (the host): a voice room for the table, everyone hearing everyone at full volume (no distances worth
    mixing by at one table; TTS has its own voice chat too, so it is off by default). Its seed is made here at load
    and only ever travels to the Koetamas, so nobody else can work out the room.

Chat commands (hidden from chat; each player's own):
  !talk ptt | always | off     how your microphone listens (default: push to talk on scripting button 1)
  !lang en                     the language you speak (Koetama's codes, or "auto")
  !koetama                     the link's state, what your chat is translated into, and your join code again
  !voice on | off              (the host only) the table's voice room

Install: in Koetama "Add game mod..." -> examples/profiles/tabletop-simulator-koetama.json, pick it; in TTS:
Modding > Scripting > Global, paste this file, Save & Play. Lua here is MoonSharp (Lua 5.2-ish): no goto, no utf8
library, no integer type.
]]

local PORT = 47160                 -- the profile's
local URL = "http://127.0.0.1:" .. PORT .. "/"
local TALK_BUTTON = 1              -- scripting button 1 = numpad 1
local WAIT = 1                     -- long poll: the answer waits up to 1 s for an object
local LINE_TTL = 10                -- seconds a chat line waits for its translation
local MAX_LINES = 16               -- lines in one player's feed (the protocol's limit)
local MAX_BYTES = 400              -- bytes of one line (the protocol's limit)
local TRANSLATION_COLOR = {0.65, 0.85, 1.0}
local LANG_NAMES = {en = "English", es = "Spanish", fr = "French", de = "German", it = "Italian", pt = "Portuguese",
    nl = "Dutch", pl = "Polish", ru = "Russian", uk = "Ukrainian", tr = "Turkish", ja = "Japanese", ko = "Korean",
    zh = "Chinese", ar = "Arabic", hi = "Hindi", sv = "Swedish", cs = "Czech"}
local INFO_COLOR = {0.6, 0.6, 0.6}
local CODE_COLOR = {1.0, 0.85, 0.4}

-- What Koetama is told for one player (the host's at the top of the feed, each other's in "players").
local function new_state(id)
    return {
        id = id,              -- steam_id (a string), nil for the host until known
        name = "",
        listen = "push_to_talk",
        talk_key = false,
        lang = "en",
        into = nil,           -- what their Koetama translates chat into ("": off; nil: not said yet)
        states = {},          -- "from>to" -> state ("ready", "downloading", ...), the pairs in use
        last_progress = {},   -- "from>to" -> the last download percentage shown
        retry = {},           -- lines answered "" while a model was not ready: sent again once ready
        status = "",          -- the last status shown ("speech/microphone")
        voice = "",           -- the voice chat's state
        voice_players = {},   -- the players whose Koetama is in the room, as this player's Koetama last said
    }
end

local kt = {
    session = 0,          -- made at load: a new session for Koetama (its numbers start over)
    ack = 0,              -- the last object number we have
    polling = false,      -- the long poll is in flight
    urgent = false,       -- an urgent (wait 0) feed is in flight
    urgent_again = false, -- something changed while it was: send another when it lands
    online = nil,         -- nil: not asked yet; true/false: the last request's outcome
    features = {},
    host = new_state(nil),
    players = {},         -- steam_id -> state, everyone seated now or before (settings kept when they leave)
    codes = {},           -- steam_id -> their join code
    joined = {},          -- steam_id -> true while their Koetama is joined
    lines = {},           -- id -> {text=, owner=state, at=, retried=}: waiting for their translation
    next_id = 1,
    voice = false,        -- the table's voice room
    seed = "",
}

-- ------------------------------------------------------------------ small helpers

local function now()
    return Time.time
end

local function all_players()
    local all = {}
    for _, list in ipairs({Player.getPlayers(), Player.getSpectators and Player.getSpectators() or {}}) do
        for _, p in ipairs(list) do all[#all + 1] = p end
    end
    return all
end

local function host()
    for _, p in ipairs(all_players()) do
        if p.host then return p end
    end
    return nil
end

local function player_by_id(id)
    for _, p in ipairs(all_players()) do
        if tostring(p.steam_id) == id then return p end
    end
    return nil
end

-- The state of a TTS player (the host's, or another's, made the first time).
local function state_of(p)
    if p.host then
        kt.host.id, kt.host.name = tostring(p.steam_id), p.steam_name or ""
        return kt.host
    end
    local id = tostring(p.steam_id)
    local st = kt.players[id]
    if not st then
        st = new_state(id)
        kt.players[id] = st
    end
    st.name = p.steam_name or ""
    return st
end

-- The state an object of Koetama's is for: its "player", or the host's (none).
local function state_for(id)
    if id == nil then return kt.host end
    return kt.players[tostring(id)]
end

local function player_of(st)
    if st == kt.host then return host() end
    return player_by_id(st.id)
end

local function name_of(st)
    local p = player_of(st)
    if p and p.steam_name then return p.steam_name end
    if st.name ~= "" then return st.name end
    return st == kt.host and "Host" or "a player"
end

-- To one player only (their chat).
local function to_player(st, msg, color)
    local p = player_of(st)
    if p then p.print(msg, color or INFO_COLOR) elseif st == kt.host then print(msg) end
end

local function tell(st, msg)
    to_player(st, "[Koetama] " .. msg, INFO_COLOR)
end

local function color_of(p)
    local ok, c = pcall(function() return Color.fromString(p.color) end)
    if ok and c then return c end
    return {1, 1, 1}
end

-- The other players for the hub: seated (not spectating), not the host.
local function others()
    local list = {}
    for _, p in ipairs(Player.getPlayers()) do
        if not p.host and p.steam_id and p.color ~= "Grey" then list[#list + 1] = p end
    end
    return list
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

-- ------------------------------------------------------------------ the feed

local function lines_json(st)
    local ids, out = {}, {}
    for id, line in pairs(kt.lines) do
        if line.owner == st then ids[#ids + 1] = id end
    end
    table.sort(ids)
    for _, id in ipairs(ids) do
        if #out >= MAX_LINES then break end
        out[#out + 1] = '{"id":' .. int(id) .. ',"text":' .. jstr(kt.lines[id].text) .. '}'
    end
    return "[" .. table.concat(out, ",") .. "]"
end

-- One player's fields. everyone: {id, name} of each player at the table (for the voice room).
local function fields(st, everyone)
    local parts = {
        '"listen":' .. jstr(st.listen),
        '"talk_key":' .. tostring(st.talk_key),
        '"lang":' .. jstr(st.lang),
        '"live":false',
        '"name":' .. jstr(st.name),
    }
    parts[#parts + 1] = '"to_translate":' .. lines_json(st)
    if kt.voice and st.id then
        -- (one table: everyone hears everyone, as loud as each other)
        local sp, to = {}, {}
        for _, e in ipairs(everyone) do
            if e.id ~= st.id then
                sp[#sp + 1] = '{"id":' .. jstr(e.id) .. ',"name":' .. jstr(e.name) .. ',"gain":1}'
                to[#to + 1] = jstr(e.id)
            end
        end
        parts[#parts + 1] = '"speakers":[' .. table.concat(sp, ",") .. ']'
        parts[#parts + 1] = '"to":[' .. table.concat(to, ",") .. ']'
    end
    return table.concat(parts, ",")
end

-- The feed, written by hand: TTS's JSON.encode cannot say "empty list" vs "empty object", and Koetama wants whole
-- numbers for session/ack/id ("3.0" is refused).
local function feed_json(wait)
    local h = host()
    if h then state_of(h) end
    local everyone, seated = {}, {}
    if kt.host.id then everyone[1] = {id = kt.host.id, name = kt.host.name} end
    for _, p in ipairs(others()) do
        local st = state_of(p)
        seated[#seated + 1] = st
        everyone[#everyone + 1] = {id = st.id, name = st.name}
    end
    local parts = {
        '"type":"feed"',
        '"session":' .. int(kt.session),
        '"ack":' .. int(kt.ack),
        '"wait":' .. (wait > 0 and int(wait) or "0"),
    }
    if kt.host.id then parts[#parts + 1] = '"me":' .. jstr(kt.host.id) end
    if kt.voice then parts[#parts + 1] = '"room_seed":' .. jstr(kt.seed) end
    parts[#parts + 1] = fields(kt.host, everyone)
    local ps = {}
    for _, st in ipairs(seated) do
        ps[#ps + 1] = '{"id":' .. jstr(st.id) .. "," .. fields(st, everyone) .. "}"
    end
    parts[#parts + 1] = '"players":[' .. table.concat(ps, ",") .. ']'
    return "{" .. table.concat(parts, ",") .. "}"
end

-- ------------------------------------------------------------------ Koetama's objects

-- No translation of this player's is downloading or loading (Koetama answers "" for a line that meets one).
local function all_ready(st)
    for _, state in pairs(st.states) do
        if state == "downloading" or state == "loading" then return false end
    end
    return true
end

local function lang_name(code)
    return LANG_NAMES[code] or code
end

-- What the player's Koetama translates chat into, in words.
local function into_text(st)
    if st.into == nil then return "not said yet" end
    if st.into == "" then return "off (choose a language in Koetama's window: Translate chat into)" end
    return lang_name(st.into)
end

local send_urgent -- (below)

local function queue_line(text, owner)
    local id = kt.next_id
    kt.next_id = kt.next_id + 1
    kt.lines[id] = {text = cut_utf8(text, MAX_BYTES), owner = owner, at = now(), retried = false}
    return id
end

local function show_code(st)
    local code = kt.codes[st.id or ""]
    local p = player_of(st)
    if not code or not p then return end
    broadcastToColor("Your Koetama code: " .. code .. "  (in Koetama: Join a game with a code)", p.color, CODE_COLOR)
end

-- translations_status: the player's target language ("into") and the pairs in use now, each with its state.
local function on_translations_status(st, o)
    if type(o.into) == "string" and o.into ~= st.into then
        st.into = o.into
        if o.into == "" then
            tell(st, "translation is off: choose a language in Koetama's window (Translate chat into)")
        else
            tell(st, "Koetama translates chat into " .. lang_name(o.into))
        end
    end
    local was_states = st.states
    st.states = {}
    for _, s in ipairs(o.translations or {}) do
        local key = tostring(s.from) .. ">" .. tostring(s.to)
        local was = was_states[key]
        st.states[key] = s.state
        if s.state == "downloading" then
            local pct = math.floor((s.progress or 0) * 100)
            if not st.last_progress[key] or pct >= st.last_progress[key] + 25 then
                st.last_progress[key] = pct
                tell(st, "translation " .. s.from .. " -> " .. s.to .. ": downloading " .. pct .. "%")
            end
        elseif s.state ~= was then
            tell(st, "translation " .. s.from .. " -> " .. s.to .. ": " .. s.state)
        end
    end
    if all_ready(st) and #st.retry > 0 then
        for _, line in ipairs(st.retry) do
            local id = queue_line(line.text, st)
            kt.lines[id].retried = true
        end
        st.retry = {}
        send_urgent()
    end
end

-- What a status object means for the player (nil: nothing worth saying).
local function status_text(speech, mic)
    if speech == "ready" and mic == "open" then return "Koetama ready" end
    if mic == "none" then return "Koetama: no microphone found" end
    if speech == "loading" then return "Koetama: the speech models are loading" end
    if speech == "error" then return "Koetama: speech to text failed (see Koetama's window)" end
    return nil
end

local function on_object(o)
    local t = o.type
    if t == "hello" then
        kt.features = {}
        for _, f in ipairs(o.features or {}) do kt.features[f] = true end
        tell(kt.host, "linked (Koetama " .. tostring(o.version) .. ", protocol " .. tostring(o.protocol) .. ")")
        return
    end
    local st = state_for(o.player)
    if not st then return end -- (a player this table never had)
    if t == "speech" then
        if o.kind == "final" and o.text and o.text ~= "" then
            local p = player_of(st)
            printToAll(name_of(st) .. ": " .. o.text, p and color_of(p) or {1, 1, 1})
            if kt.on_said then kt.on_said(o.text, st) end -- (tests)
        end
    elseif t == "translation" then
        local line = kt.lines[o.id]
        if not line or line.owner ~= st then return end
        kt.lines[o.id] = nil
        if o.text ~= "" then
            to_player(st, "    > " .. o.text, TRANSLATION_COLOR)
            if kt.on_translated then kt.on_translated(line.text, o.text, st) end -- (tests)
        elseif not all_ready(st) and not line.retried then
            -- ("" also means "the models are not ready": ask again once they are, under a new id)
            line.retried = true
            st.retry[#st.retry + 1] = line
        end
    elseif t == "translations_status" then
        on_translations_status(st, o)
    elseif t == "join_code" then
        kt.codes[st.id] = o.code
        show_code(st)
    elseif t == "player" then
        kt.joined[st.id] = o.joined == true or nil
        printToAll("[Koetama] " .. name_of(st) .. (o.joined and " joined with their Koetama" or "'s Koetama left"), INFO_COLOR)
        if o.joined then send_urgent() end -- (their lines to translate can go now)
    elseif t == "status" then
        local key = tostring(o.speech) .. "/" .. tostring(o.microphone)
        if key ~= st.status then
            st.status = key
            local text = status_text(o.speech, o.microphone)
            if text then tell(st, text) end
        end
    elseif t == "voice" then
        st.voice_players = o.players or {}
        if o.state ~= st.voice then
            local first = st.voice == ""
            st.voice = o.state
            if first and o.state == "off" then return end
            tell(st, "voice chat: " .. tostring(o.state) ..
                (o.state == "id_taken" and " (another player's id clashes with yours: no voice this session)" or ""))
        end
    end
    -- room: this mod gives a room_seed, so no offer is needed; talking: TTS shows its own speaking icons.
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
            tell(kt.host, "Koetama refused the feed: " .. tostring(req.text))
        elseif kt.online ~= false then
            tell(kt.host, "Koetama is not answering on port " .. PORT .. " (start it and pick Tabletop Simulator)")
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

-- Something changed now (a talk key, a chat line to translate): a second, wait-0 request, so it does not sit behind
-- the long poll for up to a second.
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
    math.randomseed(os.time() + math.floor((os.clock() % 1) * 1e6))
    kt.session = os.time() * 1000 + math.random(0, 999)
    kt.ack = 0
    -- the voice room's seed: random, made here and never shown, so only the Koetamas have it
    local r = {}
    for i = 1, 6 do r[i] = string.format("%06x", math.random(0, 0xffffff)) end
    kt.seed = "tts table " .. int(kt.session) .. " " .. table.concat(r) .. " " .. tostring({}):gsub("^table: ", "")
    poll()
end

local function state_of_color(color)
    for _, p in ipairs(all_players()) do
        if p.color == color then return state_of(p) end
    end
    return nil
end

function onScriptingButtonDown(index, color)
    local st = state_of_color(color)
    if index == TALK_BUTTON and st and st.listen == "push_to_talk" and not st.talk_key then
        st.talk_key = true
        send_urgent()
    end
end

function onScriptingButtonUp(index, color)
    local st = state_of_color(color)
    if index == TALK_BUTTON and st and st.talk_key then
        st.talk_key = false
        send_urgent()
    end
end

function onPlayerChangeColor(_)
    send_urgent() -- (a player sat down or stood up: the hub's list changes)
end

local LISTEN = {ptt = "push_to_talk", always = "always", off = "off"}

local function command(st, words)
    local cmd = words[1]
    if cmd == "!talk" then
        local l = LISTEN[words[2] or ""]
        if not l then tell(st, "!talk ptt | always | off") return true end
        st.listen, st.talk_key = l, false
        tell(st, "microphone: " .. l .. (l == "push_to_talk" and " (hold scripting button " .. TALK_BUTTON .. ")" or ""))
    elseif cmd == "!lang" and words[2] then
        st.lang = words[2]
        tell(st, "you speak: " .. st.lang)
    elseif cmd == "!voice" and st == kt.host and (words[2] == "on" or words[2] == "off") then
        if words[2] == "on" and not kt.features["voices"] and kt.online then
            tell(st, "this Koetama profile has no voices")
        end
        kt.voice = words[2] == "on"
        printToAll("[Koetama] the table's voice room is " .. (kt.voice and "on" or "off"), INFO_COLOR)
    elseif cmd == "!koetama" then
        local tr = {}
        for key, state in pairs(st.states) do tr[#tr + 1] = key:gsub(">", "->") .. " " .. tostring(state) end
        table.sort(tr)
        local link = st == kt.host and (kt.online and "linked" or "not linked")
            or (kt.joined[st.id] and "joined" or "not joined")
        tell(st, link .. ", mic " .. st.listen .. ", lang " .. st.lang .. ", voice " .. (kt.voice and st.voice or "off"))
        local pairs_ = #tr > 0 and " (" .. table.concat(tr, ", ") .. ")" or ""
        if not kt.features["translate"] and kt.online then
            tell(st, "translation: this Koetama profile does not translate")
        elseif st.into and st.into ~= "" then
            tell(st, "Koetama translates chat into " .. lang_name(st.into) .. pairs_)
        else
            tell(st, "translation: " .. into_text(st) .. pairs_)
        end
        if st ~= kt.host then show_code(st) end
    else
        return false
    end
    send_urgent()
    return true
end

function onChat(message, player)
    local from = state_of(player)
    if message:sub(1, 1) == "!" then
        local words = {}
        for w in message:gmatch("%S+") do words[#words + 1] = w:lower() end
        if command(from, words) then return false end
    end
    -- the line, to everyone whose Koetama is there and translates (not off in its window), but not its writer
    local queued = false
    local readers = {kt.host}
    for id, st in pairs(kt.players) do
        if kt.joined[id] then readers[#readers + 1] = st end
    end
    for _, st in ipairs(readers) do
        if st ~= from and st.into ~= "" and kt.online and kt.features["translate"] then
            queue_line(message, st)
            queued = true
        end
    end
    if queued then send_urgent() end
    return true
end

-- (tests reach the state through this; TTS ignores it)
KOETAMA = kt
