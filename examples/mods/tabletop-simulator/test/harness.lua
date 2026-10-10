--[[ Tabletop Simulator's Global.lua against the REAL Koetama (HTTP transport), in LuaJIT with TTS stubbed.

    bash run_test.sh            (sets KOETAMA_PROFILES_DIR, runs this)
    luajit harness.lua <koetama.exe> <work dir>

The stubs: WebRequest.custom is asynchronous like TTS's - each request is a background curl (start /b) whose answer
lands in a file that the frame loop looks for, then the callback runs on the "main thread" at the next frame (60 fps).
JSON is rxi/json.lua (MIT, vendored here only). Wait.time, Time.time, Player, printToAll, Color as TTS has them.
Koetama is started by the harness with --type, its stdin a pipe: a line written there is "said" by the host, so the
time from writing it to its line in "chat" is the full round trip.

The hub (PROTOCOL.md "Hub"): Blue is seated, so the host's feed lists them and Koetama answers a join code, which the
mod must show Blue only. A SECOND Koetama is then started with --join <that code> --type (Blue's, over the live
relay): a line typed into it must print as Blue, in Blue's colour; Blue's own translation (their Koetama's models)
must come back to Blue only; and with "!voice on" both Koetamas must meet in the table's seeded voice room.
KT_NO_HUB=1 skips that part, KT_NO_DOWNLOAD=1 the French model download.

Translation: what chat is translated into is Koetama's own setting (its window: "Translate chat into"), so this PC's
Koetama must be set to English before the run; the mod only learns it from translations_status's "into".
The offline checks (no Koetama, the objects faked): luajit offline.lua.
]]
local ffi = require("ffi")
ffi.cdef [[
int QueryPerformanceCounter(int64_t *c);
int QueryPerformanceFrequency(int64_t *f);
void Sleep(uint32_t ms);
unsigned int timeBeginPeriod(unsigned int ms);
]]
ffi.load("winmm").timeBeginPeriod(1) -- (Sleep(16) is ~31 ms at Windows' default 15.6 ms timer: frames at 60 fps)
local here = arg[0]:match("^(.*)[/\\]") or "."
package.path = here .. "/?.lua;" .. package.path
local rxi = require("json")

local KOETAMA_EXE, WORK = arg[1], arg[2]
assert(KOETAMA_EXE and WORK, "usage: luajit harness.lua <koetama.exe> <work dir>")

local qf, qc = ffi.new("int64_t[1]"), ffi.new("int64_t[1]")
ffi.C.QueryPerformanceFrequency(qf)
local function clock()
    ffi.C.QueryPerformanceCounter(qc)
    return tonumber(qc[0]) / tonumber(qf[0])
end
local T0 = clock()
local function t() return clock() - T0 end
local function log(fmt, ...) io.write(string.format("[%7.3f] " .. fmt .. "\n", t(), ...)) io.flush() end

-- ------------------------------------------------------------------ TTS stubs
local chat = {}          -- {at=, text=, color=}
local function to_chat(msg, color)
    chat[#chat + 1] = {at = t(), text = msg, color = color}
    log("CHAT  %s", msg)
end

local host_player = {color = "White", steam_name = "HostPlayer", steam_id = "76561198000000001", host = true, seated = true}
local blue_player = {color = "Blue", steam_name = "BluePlayer", steam_id = "76561198000000002", host = false, seated = true}
-- (TTS: p.print(msg, color); this works called with a dot or a colon)
for _, p in ipairs({host_player, blue_player}) do
    p.print = function(a, b, c)
        local msg, color = a, b
        if a == p then msg, color = b, c end
        log("%-5s %s", p.color:upper(), msg)
        chat[#chat + 1] = {at = t(), text = msg, color = color, private = true, to = p.color}
    end
end

Player = {getPlayers = function() return {host_player, blue_player} end}
Color = {fromString = function(s) return {name = s} end}
Time = {time = 0}
printToAll = to_chat
broadcastToAll = to_chat
function broadcastToColor(msg, color, tint)
    log("BCAST %s: %s", color, msg)
    chat[#chat + 1] = {at = t(), text = msg, color = tint, private = true, to = color, broadcast = true}
end
local answered = {}      -- translation id -> {text=, at=} (each object seen on the wire, duplicates counted)
JSON = {encode = rxi.encode, decode = function(s)
    if os.getenv("KT_TRACE") or s:find("translation", 1, true) then log("ANSWER %s", s) end
    local v = rxi.decode(s)
    for _, o in ipairs(type(v) == "table" and v.objects or {}) do
        if o.type == "translation" then
            local a = answered[o.id] or {n = 0}
            a.n, a.text, a.at = a.n + 1, o.text, a.at or t()
            answered[o.id] = a
        end
    end
    return v
end}

local timers = {}
Wait = {time = function(fn, secs, reps)
    timers[#timers + 1] = {at = t() + secs, fn = fn, every = secs, left = (reps or 1)}
    return #timers
end}

local inflight, reqn, stats = {}, 0, {requests = 0, urgent = 0, bodies = {}}
local function readfile(p)
    local f = io.open(p, "rb")
    if not f then return nil end
    local s = f:read("*a")
    f:close()
    return s
end
WebRequest = {custom = function(url, method, download, data, headers, cb)
    reqn = reqn + 1
    stats.requests = stats.requests + 1
    if data:find('"wait":0', 1, true) then stats.urgent = stats.urgent + 1 end
    stats.bodies[#stats.bodies + 1] = data
    local b, r, d = WORK .. "\\b" .. reqn .. ".json", WORK .. "\\r" .. reqn .. ".json", WORK .. "\\d" .. reqn .. ".txt"
    local f = assert(io.open(b, "wb"))
    f:write(data)
    f:close()
    local h = ""
    for k, v in pairs(headers or {}) do h = h .. ' -H "' .. k .. ": " .. v .. '"' end
    os.execute('start "" /b curl -s -m 5 -X ' .. method .. h .. ' --data-binary "@' .. b .. '" -o "' .. r ..
        '" -w "%output{' .. d .. '}%{http_code}" ' .. url)
    local inst = {is_done = false, url = url}
    inflight[#inflight + 1] = {inst = inst, b = b, r = r, d = d, cb = cb, at = t()}
    return inst
end}

local function pump_requests()
    for i = #inflight, 1, -1 do
        local q = inflight[i]
        local code = readfile(q.d)
        if code and #code >= 3 then
            table.remove(inflight, i)
            local n = tonumber(code) or 0
            local inst = q.inst
            inst.is_done, inst.response_code = true, n
            inst.text = readfile(q.r) or ""
            inst.is_error = n == 0 or n >= 400
            inst.error = inst.is_error and ("HTTP " .. n) or nil
            os.remove(q.b) os.remove(q.r) os.remove(q.d)
            q.cb(inst)
        end
    end
end

local function pump_timers()
    local now = t()
    for i = #timers, 1, -1 do
        local w = timers[i]
        if w and now >= w.at then
            w.left = w.left - 1
            if w.left <= 0 then table.remove(timers, i) else w.at = now + w.every end
            w.fn()
        end
    end
end

-- ------------------------------------------------------------------ the mod
dofile(here .. "/../Global.lua")
local kt = KOETAMA

-- a chat line typed by a player: onChat first (false: hidden), then it shows
local function player_chat(p, msg)
    if onChat(msg, p) ~= false then to_chat(p.steam_name .. ": " .. msg) end
end

-- ------------------------------------------------------------------ the scenario (a coroutine, one step a frame)
local koetama -- the pipe into Koetama's stdin
local joined  -- the pipe into Blue's Koetama's stdin (joined with the code)
local failures = {}
local function check(cond, what)
    log("%s  %s", cond and "PASS" or "FAIL", what)
    if not cond then failures[#failures + 1] = what end
end
local function sleep(s)
    local until_ = t() + s
    while t() < until_ do coroutine.yield() end
end
local function waitfor(cond, timeout)
    local until_ = t() + timeout
    while t() < until_ do
        local v = cond()
        if v then return v end
        coroutine.yield()
    end
    return nil
end
local function chat_line(pred, from)
    for i = from or 1, #chat do
        if pred(chat[i].text) then return chat[i], i end
    end
end
local function chat_entry(pred, from) -- (pred gets the whole entry: text, color, to)
    for i = from or 1, #chat do
        if pred(chat[i]) then return chat[i], i end
    end
end
local function stats_of(xs)
    table.sort(xs)
    local sum = 0
    for _, x in ipairs(xs) do sum = sum + x end
    return string.format("n=%d min %.0f ms, median %.0f ms, mean %.0f ms, max %.0f ms", #xs, xs[1] * 1000,
        xs[math.floor((#xs + 1) / 2)] * 1000, sum / #xs * 1000, xs[#xs] * 1000)
end

local SECONDS = 400 -- (Koetama's --seconds: a ceiling; it is stopped when the scenario ends)
local scenario = coroutine.create(function()
    log("load Global.lua with no Koetama running")
    onLoad("")
    check(waitfor(function() return kt.online == false end, 8), "no Koetama: the mod says so and keeps trying")

    log("start Koetama")
    -- (cmd /c drops the outer quotes: the whole line in one more pair)
    koetama = assert(io.popen('""' .. KOETAMA_EXE .. '" --cli --game tabletop-simulator-koetama --type --volume 0 --seconds ' ..
        SECONDS .. ' > "' .. WORK .. '\\koetama.log" 2>&1"', "w"))
    local t_start = t()
    check(waitfor(function() return kt.features.speech end, 20), "hello arrives (features: speech, translate)")
    log("linked %.2f s after Koetama started (the retry is every 2 s)", t() - t_start)

    -- the hub: Blue is seated, so the feed lists them, and their join code is shown to Blue only
    local body = stats.bodies[#stats.bodies]
    check(body:find('"me":"76561198000000001"', 1, true) and body:find('"players":[{"id":"76561198000000002"', 1, true) ~= nil,
        "the feed: the host's steam id as me, Blue in players")
    local code_line = waitfor(function()
        return chat_entry(function(c) return c.broadcast and c.text:find("Your Koetama code: ", 1, true) end)
    end, 10)
    local code = code_line and code_line.text:match("code: (%w%w%w%w%-%w%w%w%w)")
    check(code ~= nil and code_line.to == "Blue", "Blue's join code is shown to Blue only (" .. tostring(code) .. ")")
    check(not chat_entry(function(c) return not c.private and c.text:find(tostring(code), 1, true) end),
        "... and nowhere in the public chat")

    -- translation: Koetama's own setting (into English); the first line may meet a model that is not "ready" yet
    player_chat(host_player, "!koetama")
    check(not chat_line(function(s) return s:find("!koetama", 1, true) end), "a command is hidden from chat")
    local t_line = t()
    player_chat(blue_player, "¿Dónde está la biblioteca?")
    local got = waitfor(function() return chat_line(function(s) return s:find("^    > ") end) end, 30)
    check(got ~= nil, "the first Spanish line, sent with the command, is translated (at once, or after a \"\" and a retry)")
    if got then log("first translation: %q, %.0f ms after the line", got.text, (got.at - t_line) * 1000) end
    check(waitfor(function() return kt.host.into == "en" end, 5), "translations_status says Koetama translates into en (" ..
        tostring(kt.host.into) .. ")")
    check(chat_entry(function(e) return e.to == "White" and e.text == "[Koetama] Koetama translates chat into English" end),
        "the host is told what their chat is translated into")

    -- push to talk on the scripting button: an urgent feed each way
    local u0 = stats.urgent
    onScriptingButtonDown(1, "White")
    sleep(0.3)
    local held = stats.bodies[#stats.bodies]
    onScriptingButtonUp(1, "White")
    sleep(0.3)
    check(stats.urgent - u0 >= 2, "push to talk: the key down and up each sent a feed at once")
    check(held and held:match('^{[^%[]*"talk_key":true') ~= nil, "push to talk: the host's talk_key true while held")
    onScriptingButtonDown(1, "Blue")
    sleep(0.2)
    local blue_held = stats.bodies[#stats.bodies]
    onScriptingButtonUp(1, "Blue")
    check(kt.host.talk_key == false and blue_held:match('^{[^%[]*"talk_key":false') ~= nil,
        "Blue's scripting button is not the host's talk key")
    check(blue_held:find('"players":[{"id":"76561198000000002","listen":"push_to_talk","talk_key":true', 1, true) ~= nil,
        "... it is Blue's own, in Blue's feed")

    -- speech: lines "said" (typed into Koetama) at random points of the long poll; time to their chat line
    local said = {"hello there everyone", "roll the dice please", "I will trade two wheat for one ore",
        "your turn", "who has the longest road", "good game", "wait I need to shuffle", "nice move",
        "pass the cards", "one more round"}
    local lat = {}
    for _, line in ipairs(said) do
        sleep(0.2 + math.random() * 1.1)
        local mark = #chat + 1
        local t_said = t()
        koetama:write(line .. "\n")
        koetama:flush()
        local c = waitfor(function()
            return chat_line(function(s) return s == "HostPlayer: " .. line end, mark)
        end, 5)
        if c then lat[#lat + 1] = c.at - t_said end
    end
    check(#lat == #said, "every line said shows in chat as the host (" .. #lat .. "/" .. #said .. ")")
    if #lat > 0 then log("LATENCY speech (typed -> chat): %s", stats_of(lat)) end

    -- translation round trips, the model ready
    local es = {"Me toca a mí", "Tengo dos ovejas", "Buena suerte a todos", "¿Quién tiene los dados?",
        "Vamos a jugar otra vez"}
    es[#es + 1] = "¿Me pasas las cartas, por favor?"
    es[#es + 1] = "Creo que vamos a perder esta partida"
    es[#es + 1] = "El dragón está en la montaña"
    local tl, shown, empty = {}, 0, {}
    for _, line in ipairs(es) do
        sleep(0.2 + math.random() * 1.1)
        local id = kt.next_id
        local t_chat = t()
        player_chat(blue_player, line)
        local a = waitfor(function() return answered[id] end, 10)
        if a then
            tl[#tl + 1] = a.at - t_chat
            if a.text ~= "" then shown = shown + 1 else empty[#empty + 1] = line end
        end
        sleep(0.1)
    end
    check(#tl == #es, "every Spanish line is answered exactly once (" .. #tl .. "/" .. #es .. ")")
    if #tl > 0 then log("LATENCY translation (chat -> translation object): %s", stats_of(tl)) end
    log("KOETAMA translated %d/%d Spanish lines; \"\" for: %s", shown, #es, table.concat(empty, " | "))

    -- an English line: "" back (nothing in Spanish), nothing printed, the line dropped
    local mark = #chat + 1
    player_chat(blue_player, "ok sounds good")
    sleep(1.5)
    check(not chat_line(function(s) return s:find("^    > ") end, mark), "a line with no Spanish: nothing printed")
    check(next(kt.lines) == nil, "no line left waiting for a translation")

    -- the hub: Blue's Koetama joins with the code; what Blue says prints as Blue
    if os.getenv("KT_NO_HUB") == nil and code then
        log("start Blue's Koetama: --join %s", code)
        local t_join = t()
        joined = assert(io.popen('""' .. KOETAMA_EXE .. '" --cli --join ' .. code .. ' --type --volume 0 --seconds ' ..
            SECONDS .. ' > "' .. WORK .. '\\joined.log" 2>&1"', "w"))
        local j = waitfor(function()
            return chat_line(function(s) return s == "[Koetama] BluePlayer joined with their Koetama" end)
        end, 40)
        check(j ~= nil, "everyone is told Blue's Koetama joined")
        if j then log("Blue's Koetama joined %.1f s after it started (the relay included)", j.at - t_join) end
        check(waitfor(function() return kt.joined["76561198000000002"] end, 1), "the mod knows Blue is joined")
        local said_blue = {"hello from blue", "I have the longest road", "can someone trade me sheep"}
        local bl = {}
        for _, line in ipairs(said_blue) do
            sleep(0.3 + math.random() * 1.0)
            local mark = #chat + 1
            local t_said = t()
            joined:write(line .. "\n")
            joined:flush()
            local c = waitfor(function()
                return chat_entry(function(e) return e.text == "BluePlayer: " .. line and not e.private end, mark)
            end, 10)
            if c then
                bl[#bl + 1] = c.at - t_said
                check(type(c.color) == "table" and c.color.name == "Blue", "\"" .. line .. "\" prints as Blue, in Blue's colour")
            end
        end
        check(#bl == #said_blue, "every line Blue says (typed into Blue's Koetama) prints as Blue (" .. #bl .. "/" .. #said_blue .. ")")
        if #bl > 0 then log("LATENCY Blue's speech (typed into the joined Koetama -> host's chat, over the relay): %s", stats_of(bl)) end
        check(not chat_line(function(s) return s:find("HostPlayer: hello from blue", 1, true) end),
            "Blue's lines are never the host's")
        local st = waitfor(function()
            return chat_entry(function(e) return e.to == "Blue" and e.text:find("^%[Koetama%] Koetama") end)
        end, 10)
        check(st ~= nil, "Blue is told their Koetama's status (" .. (st and st.text or "nothing") .. ")")

        -- Blue's own translation, made by Blue's Koetama (its own setting), shown to Blue only
        check(waitfor(function() return kt.players["76561198000000002"].into == "en" end, 10),
            "Blue's Koetama says it translates into en")
        local m2 = #chat + 1
        local t_es = t()
        player_chat(host_player, "¿Alguien tiene madera para cambiar?")
        local tb = waitfor(function()
            return chat_entry(function(e) return e.to == "Blue" and e.text:find("^    > ") end, m2)
        end, 60)
        check(tb ~= nil, "Blue's translation of the host's Spanish line reaches Blue")
        if tb then log("Blue's translation: %q, %.0f ms after the line", tb.text, (tb.at - t_es) * 1000) end
        check(not chat_entry(function(e) return e.to ~= "Blue" and e.text:find("^    > ") end, m2),
            "... and only Blue (the host does not translate their own line)")

        -- the table's voice room: both Koetamas meet in the room made from the seed
        player_chat(host_player, "!voice on")
        local blue = kt.players["76561198000000002"]
        local function has(list, id)
            for _, x in ipairs(list or {}) do if tostring(x) == id then return true end end
            return false
        end
        local v = waitfor(function()
            return kt.host.voice == "connected" and has(kt.host.voice_players, "76561198000000002")
                and has(blue.voice_players, "76561198000000001")
        end, 40)
        check(v ~= nil, "!voice on: the host's and Blue's Koetamas meet in the table's room (voice " ..
            kt.host.voice .. " / " .. blue.voice .. ")")
        player_chat(host_player, "!voice off")
        mark = #chat + 1
    end

    -- a language whose model is not on this PC yet (fr -> en, ~35 MB the first time, fetched on first sight of French):
    -- the line waits for it, or comes back "" and is sent again under a new id once translations_status says ready
    if os.getenv("KT_NO_DOWNLOAD") == nil then
        local id = kt.next_id
        local t_fr = t()
        player_chat(blue_player, "Est-ce que quelqu'un veut échanger du bois contre du blé ?")
        local first = waitfor(function() return answered[id] end, 10)
        check(first ~= nil, "fr: the line is answered while the model is missing (" ..
            (first and string.format("%q", first.text) or "nothing") .. ")")
        local c = waitfor(function() return chat_line(function(s) return s:find("^    > ") end, mark) end, 240)
        check(c ~= nil, "fr: once ready, the line sent again is translated")
        if c then log("fr: translation shown %.1f s after the line (download and load included): %s", c.at - t_fr, c.text) end
    end

    -- each object handled once: no line printed twice despite two requests in flight at times
    local seen, dup = {}, false
    for _, c in ipairs(chat) do
        if not c.private then
            if seen[c.text] and c.text:find("^HostPlayer: ") then dup = true end
            seen[c.text] = true
        end
    end
    check(not dup, "no object handled twice")
    log("requests: %d (%d urgent), last object %d", stats.requests, stats.urgent, kt.ack)
end)

-- ------------------------------------------------------------------ the frame loop
local frames, f_start = 0, t()
while coroutine.status(scenario) ~= "dead" do
    frames = frames + 1
    Time.time = t()
    pump_requests()
    pump_timers()
    local ok, err = coroutine.resume(scenario)
    if not ok then failures[#failures + 1] = "scenario error: " .. tostring(err) log("ERROR %s", err) break end
    ffi.C.Sleep(16)
end
if joined then
    os.execute([[wmic process where "name='koetama.exe' and commandline like '%%--join%%'" call terminate >nul 2>&1]])
    joined:close()
end
if koetama then
    -- (only the Koetama this test started: its command line names this profile)
    os.execute([[wmic process where "name='koetama.exe' and commandline like '%%tabletop-simulator-koetama%%'" call terminate >nul 2>&1]])
    koetama:close()
end
log("frames: %d, %.1f ms each", frames, (t() - f_start) / frames * 1000)
log("%s: %d failure(s)", #failures == 0 and "OK" or "FAILED", #failures)
os.exit(#failures == 0 and 0 or 1)
