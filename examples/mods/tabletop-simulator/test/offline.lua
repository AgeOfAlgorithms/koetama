--[[ Tabletop Simulator's Global.lua with NO Koetama: TTS stubbed, Koetama's answers faked. luajit offline.lua

Checks the translation side: the feed carries the chat lines (to_translate) and never a "translations" list, what the
chat is translated into comes from translations_status's "into" (Koetama's own setting), a player whose Koetama says
it is off gets no lines, and a line answered "" while a model downloads is sent again once it is ready.
]]
local here = arg and arg[0] and arg[0]:match("^(.*)[/\\]") or "."
package.path = here .. "/?.lua;" .. package.path
local rxi = require("json")

local chat = {} -- {text=, to=}
local function to_chat(msg) chat[#chat + 1] = {text = msg} end
local host_player = {color = "White", steam_name = "HostPlayer", steam_id = "76561198000000001", host = true}
local blue_player = {color = "Blue", steam_name = "BluePlayer", steam_id = "76561198000000002", host = false}
for _, p in ipairs({host_player, blue_player}) do
    p.print = function(a, b)
        local msg = a
        if a == p then msg = b end
        chat[#chat + 1] = {text = msg, to = p.color}
    end
end
Player = {getPlayers = function() return {host_player, blue_player} end}
Color = {fromString = function(s) return {name = s} end}
Time = {time = 0}
printToAll = to_chat
function print(msg) chat[#chat + 1] = {text = msg, to = "console"} end
function broadcastToColor(msg, color) chat[#chat + 1] = {text = msg, to = color} end
JSON = {encode = rxi.encode, decode = rxi.decode}
Wait = {time = function() end}

local pending, bodies, last = {}, {}, 0
WebRequest = {custom = function(_, _, _, data, _, cb)
    bodies[#bodies + 1] = data
    pending[#pending + 1] = cb
end}

-- Koetama answers the oldest request in flight with these objects (numbered on from the last).
local function answer(objects)
    local cb = table.remove(pending, 1)
    assert(cb, "no request in flight")
    last = last + #objects
    cb({response_code = 200, text = rxi.encode({objects = objects, last = last})})
end

dofile(here .. "/../Global.lua")
local kt = KOETAMA

local failures, passes = {}, 0
local function check(cond, what)
    io.write((cond and "PASS  " or "FAIL  ") .. what .. "\n")
    if cond then passes = passes + 1 else failures[#failures + 1] = what end
end
local function told(to, text, from)
    for i = from or 1, #chat do
        if chat[i].to == to and chat[i].text == text then return chat[i] end
    end
end
local function told_like(to, pattern, from)
    for i = from or 1, #chat do
        if chat[i].to == to and chat[i].text:find(pattern) then return chat[i] end
    end
end
-- A feed sent since body number `from` has all these pieces (answering the requests in flight, so the next go).
local function sent(from, ...)
    local want = {...}
    for _ = 1, 6 do
        for i = from, #bodies do
            local all = true
            for _, w in ipairs(want) do all = all and bodies[i]:find(w, 1, true) ~= nil end
            if all then return true end
        end
        if #pending == 0 then return false end
        answer({})
    end
    return false
end
local function player_chat(p, msg)
    if onChat(msg, p) ~= false then to_chat(p.steam_name .. ": " .. msg) end
end

onLoad("")
check(#pending == 1, "onLoad starts the long poll")
answer({{type = "hello", app = "Koetama", version = "0.5.0", protocol = 2, features = {"speech", "voices", "translate"}}})
check(kt.features.translate and kt.online, "hello read: online, features")

-- before any translations_status: lines still go (Koetama answers "" when it is off)
local mark = #chat + 1
local id0 = kt.next_id
player_chat(blue_player, "hola a todos")
check(kt.lines[id0] and kt.lines[id0].owner == kt.host, "into not said yet: Blue's line is queued for the host")
check(bodies[#bodies]:find('"to_translate":[{"id":' .. id0 .. ',"text":"hola a todos"}]', 1, true) ~= nil,
    "the line is in the host's feed (to_translate)")
for _, b in ipairs(bodies) do
    if b:find('"translations"', 1, true) then check(false, "no feed carries \"translations\"") break end
end
check(not kt.lines[id0 + 1], "Blue (not joined) gets no line; the writer never their own")

-- translations_status with "into"
answer({{type = "translations_status", into = "en", translations = {{from = "es", to = "en", state = "ready"}}}})
check(kt.host.into == "en", "into read from translations_status")
check(told("White", "[Koetama] Koetama translates chat into English", mark), "the host is told: into English")
check(kt.host.states["es>en"] == "ready", "the pair's state kept")

answer({{type = "translation", id = id0, text = "hello everyone", from = "es", to = "en"}})
check(told("White", "    > hello everyone", mark), "the translation is printed to the host only, under the line")
check(not told("Blue", "    > hello everyone"), "... not to Blue")
check(kt.lines[id0] == nil, "the line is dropped once answered")

-- !translate is gone: it is plain chat now; !koetama says what chat is translated into
mark = #chat + 1
player_chat(host_player, "!translate es en")
check(told(nil, "HostPlayer: !translate es en", mark), "!translate is no command any more (it shows as chat)")
mark = #chat + 1
player_chat(host_player, "!koetama")
check(told("White", "[Koetama] Koetama translates chat into English (es->en ready)", mark),
    "!koetama: Koetama translates chat into English, with the pairs' states")
check(not told(nil, "HostPlayer: !koetama", mark), "!koetama is hidden from chat")

-- a model downloading: progress shown; a line answered "" meanwhile is sent again once ready
mark = #chat + 1
answer({{type = "translations_status", into = "en", translations = {
    {from = "es", to = "en", state = "ready"}, {from = "fr", to = "en", state = "downloading", progress = 0.42}}}})
check(told("White", "[Koetama] translation fr -> en: downloading 42%", mark), "download progress shown")
local id1 = kt.next_id
player_chat(blue_player, "bonjour tout le monde")
answer({{type = "translation", id = id1, text = ""}})
check(kt.lines[id1] == nil and #kt.host.retry == 1, "\"\" while downloading: kept to send again")
local id2 = kt.next_id
answer({{type = "translations_status", into = "en", translations = {
    {from = "es", to = "en", state = "ready"}, {from = "fr", to = "en", state = "ready"}}}})
check(kt.lines[id2] and kt.lines[id2].text == "bonjour tout le monde" and #kt.host.retry == 0,
    "once ready: the line is sent again under a new id")
answer({{type = "translation", id = id2, text = "hello everybody", from = "fr", to = "en"}})
check(told("White", "    > hello everybody", mark), "... and its translation is shown")

-- off in Koetama's window: no lines for that player
mark = #chat + 1
answer({{type = "translations_status", into = "", translations = {}}})
check(kt.host.into == "", "into \"\": off")
check(told("White", "[Koetama] translation is off: choose a language in Koetama's window (Translate chat into)", mark),
    "the host is told translation is off, and where to choose")
check(next(kt.host.states) == nil, "the pairs in use: none")
local id3 = kt.next_id
player_chat(blue_player, "otra línea")
check(kt.lines[id3] == nil, "off: Blue's line is not queued for the host")
player_chat(host_player, "!koetama")
check(told_like("White", "^%[Koetama%] translation: off %(choose a language", mark), "!koetama: translation off")

-- Blue joins: their own Koetama, their own setting
mark = #chat + 1
answer({{type = "player", player = "76561198000000002", joined = true}})
answer({{type = "translations_status", player = "76561198000000002", into = "ja", translations = {}}})
local blue = kt.players["76561198000000002"]
check(blue.into == "ja", "Blue's into read from their own translations_status")
check(told("Blue", "[Koetama] Koetama translates chat into Japanese", mark), "Blue is told: into Japanese")
local id4, b4 = kt.next_id, #bodies + 1
player_chat(host_player, "hello blue")
check(kt.lines[id4] and kt.lines[id4].owner == blue and not kt.lines[id4 + 1],
    "the host's line goes to Blue only (the host's translation is off)")
check(sent(b4, '"players":[{"id":"76561198000000002"', '"to_translate":[{"id":' .. id4 .. ',"text":"hello blue"}]}]'),
    "... in Blue's part of the feed")
answer({{type = "translation", player = "76561198000000002", id = id4, text = "こんにちは", from = "en", to = "ja"}})
check(told("Blue", "    > こんにちは", mark) and not told("White", "    > こんにちは", mark), "Blue's translation to Blue only")

io.write(string.format("%s: %d passed, %d failed\n", #failures == 0 and "OK" or "FAILED", passes, #failures))
os.exit(#failures == 0 and 0 or 1)
