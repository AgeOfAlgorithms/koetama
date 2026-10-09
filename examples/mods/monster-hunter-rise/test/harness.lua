--[[ Monster Hunter Rise's koetama.lua against the REAL Koetama (files transport), in LuaJIT with REFramework stubbed.

    bash run_test.sh            (sets KOETAMA_PROFILES_DIR, SAVEPROBE_DIR, HFP_MODS; runs this)
    luajit harness.lua <koetama.exe> <work dir>

<work>/data plays reframework/data. The stubs do what REFramework's do (src/mods/bindings/Json.cpp, FS.cpp):
json.dump_file truncates and writes in place (not atomic) with an empty table as null, json.load_string returns nil
on bad JSON, fs.read returns "" for a missing file. re.on_frame runs every frame (60 fps); imgui calls are recorded
as the overlay's text; reframework:is_key_down reads a table the scenario sets.
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
local DATA = WORK .. "\\data\\"

local qf, qc = ffi.new("int64_t[1]"), ffi.new("int64_t[1]")
ffi.C.QueryPerformanceFrequency(qf)
local function clock()
    ffi.C.QueryPerformanceCounter(qc)
    return tonumber(qc[0]) / tonumber(qf[0])
end
local T0 = clock()
local function t() return clock() - T0 end
local function log(fmt, ...) io.write(string.format("[%7.3f] " .. fmt .. "\n", t(), ...)) io.flush() end
os.clock = t -- (Windows' clock() is wall time since start, as REFramework's Lua sees it)

local function readfile(p)
    local f = io.open(p, "rb")
    if not f then return nil end
    local s = f:read("*a")
    f:close()
    return s
end
local function path(rel) return DATA .. rel:gsub("/", "\\") end

-- ------------------------------------------------------------------ REFramework stubs
-- nlohmann/json as REFramework's encode_any makes it: integers as integers, a sequence as an array, an empty table null
local function encode(v)
    local ty = type(v)
    if v == nil then return "null" end
    if ty == "boolean" then return tostring(v) end
    if ty == "number" then
        if v == math.floor(v) and math.abs(v) < 2 ^ 53 then return string.format("%.0f", v) end
        return string.format("%.17g", v)
    end
    if ty == "string" then return rxi.encode(v) end
    if ty == "table" then
        if next(v) == nil then return "null" end
        local n = 0
        for _ in pairs(v) do n = n + 1 end
        if n == #v then
            local out = {}
            for i = 1, #v do out[i] = encode(v[i]) end
            return "[" .. table.concat(out, ",") .. "]"
        end
        local out = {}
        for k, x in pairs(v) do out[#out + 1] = rxi.encode(tostring(k)) .. ":" .. encode(x) end
        return "{" .. table.concat(out, ",") .. "}"
    end
    return "null"
end

local writes = {count = 0, last = nil}
local load_file_errors = 0
json = {
    dump_file = function(rel, v, indent)
        local f = io.open(path(rel), "wb")
        if not f then
            os.execute('mkdir "' .. path(rel):match("^(.*)\\") .. '" 2>nul')
            f = assert(io.open(path(rel), "wb"))
        end
        local s = encode(v)
        f:write(s)
        f:close()
        writes.count, writes.last = writes.count + 1, s
        return true
    end,
    load_file = function(rel)
        local s = readfile(path(rel))
        local ok, v = pcall(rxi.decode, s or "")
        if not ok then load_file_errors = load_file_errors + 1 return nil end
        return v
    end,
    load_string = function(s)
        local ok, v = pcall(rxi.decode, s)
        return ok and v or nil
    end,
    dump_string = function(v) return encode(v) end,
}
fs = {
    read = function(rel) return readfile(path(rel)) or "" end,
    write = function(rel, s) local f = assert(io.open(path(rel), "wb")) f:write(s) f:close() end,
    glob = function() error("not stubbed") end,
}
local keys = {}
reframework = {is_key_down = function(self, vk) return keys[vk] == true end}
local on_frame, on_draw_ui
re = {on_frame = function(f) on_frame = f end, on_draw_ui = function(f) on_draw_ui = f end}
local overlay = {}        -- this frame's overlay lines
imgui = {
    begin_window = function() overlay = {} return true end,
    end_window = function() end,
    text = function(s) overlay[#overlay + 1] = s end,
    text_colored = function(s) overlay[#overlay + 1] = s end,
    tree_node = function() return false end,
}
sdk = nil

-- ------------------------------------------------------------------ the mod
dofile(here .. "/../reframework/autorun/koetama.lua")
local kt, cfg = Koetama.state, Koetama.config

-- ------------------------------------------------------------------ the scenario
local koetama
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
local seen_at = {}        -- overlay text -> when it first showed
local function on_screen(s) return seen_at[s] end
local function stats_of(xs)
    table.sort(xs)
    local sum = 0
    for _, x in ipairs(xs) do sum = sum + x end
    return string.format("n=%d min %.0f ms, median %.0f ms, mean %.0f ms, max %.0f ms", #xs, xs[1] * 1000,
        xs[math.floor((#xs + 1) / 2)] * 1000, sum / #xs * 1000, xs[#xs] * 1000)
end
local function message_files()
    local out = {}
    local p = io.popen('dir /b "' .. DATA .. 'koetama" 2>nul')
    for name in p:lines() do
        local n = name:match("^kt_t(%d+)%.json$")
        if n then out[#out + 1] = tonumber(n) end
    end
    p:close()
    table.sort(out)
    return out
end

local scenario = coroutine.create(function()
    log("frames with no Koetama running")
    sleep(1)
    check(readfile(path("koetama/feed.json")) ~= nil, "the feed file is written (folder made by dump_file)")
    check(on_screen("Koetama is not running (start it, then pick Monster Hunter Rise)"), "overlay: Koetama not running")
    local w0, t0 = writes.count, t()
    sleep(2)
    log("feed writes with nothing happening: %.1f a second", (writes.count - w0) / (t() - t0))

    log("start Koetama")
    koetama = assert(io.popen('""' .. KOETAMA_EXE .. '" --cli --game monster-hunter-rise-koetama --type --volume 0 ' ..
        '--seconds 400 > "' .. WORK .. '\\koetama.log" 2>&1"', "w"))
    local t_start = t()
    check(waitfor(function() return kt.features.speech end, 20), "hello arrives through kt_t1.json")
    log("linked %.2f s after Koetama started", t() - t_start)
    check(waitfor(function() return kt.online end, 3), "kt_on and the ping's answer: online")

    -- push to talk: the feed file says talk_key within a frame
    keys[0x56] = true
    local tk = waitfor(function()
        local s = readfile(path("koetama/feed.json")) or ""
        return s:find('"talk_key":true', 1, true) and t()
    end, 1)
    local t_down = t()
    check(tk ~= nil, "push to talk: V held -> talk_key true in the feed file")
    keys[0x56] = nil
    sleep(0.2)
    check((readfile(path("koetama/feed.json")) or ""):find('"talk_key":false', 1, true), "push to talk: released")

    -- translation (es -> en; the first line meets the model loading)
    cfg.translations = {{from = "es", to = "en"}}
    Koetama.translate("¿Dónde está la biblioteca?", "Hunter2")
    local tr = waitfor(function() return on_screen("Hunter2: Where is the library?") end, 30)
    check(tr ~= nil, "a translation shows in the overlay")
    local tl = {}
    for _, line in ipairs({"¿Quién tiene la gran espada?", "El dragón está en la montaña", "¿Me ayudas con el monstruo?"}) do
        sleep(0.2 + math.random() * 0.5)
        local mark = t()
        local id = Koetama.translate(line, "Hunter2")
        local got = waitfor(function() return kt.lines[id] == nil and t() end, 10)
        if got then tl[#tl + 1] = got - mark end
    end
    check(#tl == 3, "every line to translate is answered (" .. #tl .. "/3)")
    if #tl > 0 then log("LATENCY translation (queued -> translation read): %s", stats_of(tl)) end

    -- speech: lines typed into Koetama, timed to the overlay
    local said = {"carve it before it disappears", "I need a farcaster", "trap is set", "monster is limping",
        "nice wyvern riding", "careful it is enraged", "cart incoming", "heal me please", "break the horn",
        "good hunt everyone"}
    local lat = {}
    for _, line in ipairs(said) do
        sleep(0.2 + math.random() * 1.0)
        local t_said = t()
        koetama:write(line .. "\n")
        koetama:flush()
        local at = waitfor(function() return on_screen(line) end, 5)
        if at then lat[#lat + 1] = at - t_said end
    end
    check(#lat == #said, "every line said shows in the overlay (" .. #lat .. "/" .. #said .. ")")
    if #lat > 0 then log("LATENCY speech (typed -> overlay): %s", stats_of(lat)) end

    -- acks: Koetama deletes what was read
    sleep(0.5)
    local left = message_files()
    local stale = 0
    for _, n in ipairs(left) do if n <= kt.ack then stale = stale + 1 end end
    check(stale == 0, string.format("acked message files are deleted (%d left, %d of them acked; last read %d)",
        #left, stale, kt.ack))
    check(load_file_errors == 0, "no json.load_file misses (fs.read first)")
    local w1, t1 = writes.count, t()
    sleep(2)
    log("feed writes while linked: %.1f a second (%d writes in total)", (writes.count - w1) / (t() - t1), writes.count)

    -- Koetama killed (a crash: kt_on stays): the ping goes unanswered and the overlay says so
    log("kill Koetama")
    os.execute([[wmic process where "name='koetama.exe' and commandline like '%%monster-hunter-rise-koetama%%'" call terminate >nul 2>&1]])
    koetama:close()
    koetama = nil
    local t_kill = t()
    local gone = waitfor(function() return not kt.online and t() end, 10)
    check(gone ~= nil, "a killed Koetama is noticed (no ping answer)")
    if gone then log("noticed %.1f s after the kill (kt_on %s)", gone - t_kill,
        readfile(path("koetama/kt_on")) and "left behind" or "gone") end
end)

-- ------------------------------------------------------------------ the frame loop
local frames, f_start = 0, t()
while coroutine.status(scenario) ~= "dead" do
    frames = frames + 1
    on_frame()
    for _, s in ipairs(overlay) do
        if not seen_at[s] then
            seen_at[s] = t()
            log("SHOW  %s", s)
        end
    end
    local ok, err = coroutine.resume(scenario)
    if not ok then failures[#failures + 1] = "scenario error: " .. tostring(err) log("ERROR %s", err) break end
    ffi.C.Sleep(16)
end
if koetama then
    os.execute([[wmic process where "name='koetama.exe' and commandline like '%%monster-hunter-rise-koetama%%'" call terminate >nul 2>&1]])
    koetama:close()
end
log("frames: %d, %.1f ms each", frames, (t() - f_start) / frames * 1000)
log("%s: %d failure(s)", #failures == 0 and "OK" or "FAILED", #failures)
os.exit(#failures == 0 and 0 or 1)
