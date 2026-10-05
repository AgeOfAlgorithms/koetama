-- Offline stand-in for the game (LuaJIT), to test probe.py without it: runs the installed mod's main.lua
-- at ~60 fps; HasFile asks the real disk (MOD/ = the installed mod), except MOD/../ (never seen) and
-- absolute paths (an error), to exercise those lines of the report. Writes <dir>/savegame.xml each frame.
--   luajit probes/hasfileprobe/sim.lua <dir> <installed mod folder>     (phases 12 / 2 / 10 s)
local ffi = require("ffi")
ffi.cdef[[ void Sleep(uint32_t ms); int QueryPerformanceCounter(int64_t *c); int QueryPerformanceFrequency(int64_t *f); ]]
local dir, mod = arg[1], arg[2]
local c, f = ffi.new("int64_t[1]"), ffi.new("int64_t[1]")
ffi.C.QueryPerformanceFrequency(f)
local function clock() ffi.C.QueryPerformanceCounter(c) return tonumber(c[0]) / tonumber(f[0]) end
local reg, order = {}, {}
local function set(k, v) if reg[k] == nil then order[#order + 1] = k end reg[k] = tostring(v) end
SetString, SetInt = set, set
function ClearKey() reg, order = {}, {} end
function HasFile(path)
	if path:find("^MOD/%.%./") then return false end
	if not path:find("^MOD/") then error("no absolute paths") end
	local fh = io.open(mod .. path:sub(4), "rb")
	if fh then fh:close() return true end
	return false
end
function LoadSound(path) return HasFile(path) and 7 or 0 end
function PlaySound(h) print("PlaySound " .. tostring(h)) end
function GetCameraTransform() return {pos = {0, 0, 0}} end
local t0 = clock()
function GetTime() return clock() - t0 end
local function flush()
	local out = {'<registry version="2.1.0">\n<savegame><mod><local-hasfile-probe><hfp>\n'}
	for _, k in ipairs(order) do out[#out + 1] = string.format('<%s value="%s"/>\n', k:match("([^.]+)$"), reg[k]) end
	out[#out + 1] = "</hfp></local-hasfile-probe></mod></savegame>\n</registry>\n"
	local fh = io.open(dir .. "/savegame.xml", "wb")
	if fh then fh:write(table.concat(out)); fh:close() end
end
client = {}
assert(loadstring(assert(io.open(mod .. "/abs.lua")):read("*a")))()
local src = assert(io.open(mod .. "/main.lua")):read("*a")
src = src:gsub("^#version 2", "--"):gsub('#include "abs.lua"', "--"):gsub("local BASE, HEAVY, SOUND = 25, 10, 14", "local BASE, HEAVY, SOUND = 12, 2, 10")
assert(loadstring(src, "main.lua"))()
client.init()
while true do
	client.tick(1 / 60)
	flush()
	if client.phase == "done" and GetTime() > 26 then break end
	ffi.C.Sleep(15)
end
