-- Offline stand-in for the game (LuaJIT), to test probe.py without it: runs the installed mod's main.lua
-- at ~60 fps; HasFile asks the real disk; sounds are only counted. Writes <dir>/savegame.xml each frame.
--   luajit probes/voiceprobe/sim.lua <dir> <installed mod folder>
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
	local fh = io.open(mod .. path:sub(4), "rb")
	if fh then fh:close() return true end
	return false
end
local loads, plays, unloads, skips = 0, 0, 0, 0
function LoadSound(path) assert(HasFile(path)); loads = loads + 1; return loads end
function PlaySound(h, pos) assert(pos and pos[1]); plays = plays + 1; return plays end
function SetSoundProgress(h, s) assert(s >= 0 and s <= 0.03); skips = skips + 1 end
function UnloadSound(h) unloads = unloads + 1 end
function Vec(x, y, z) return {x or 0, y or 0, z or 0} end
function VecAdd(a, b) return {a[1] + b[1], a[2] + b[2], a[3] + b[3]} end
function VecScale(a, s) return {a[1] * s, a[2] * s, a[3] * s} end
function VecNormalize(a) local l = math.sqrt(a[1] ^ 2 + a[2] ^ 2 + a[3] ^ 2); return {a[1] / l, a[2] / l, a[3] / l} end
function TransformToParentVec(t, v) return v end
function GetCameraTransform() return {pos = {0, 1.7, 0}} end
local t0 = clock()
function GetTime() return clock() - t0 end
local function flush()
	local out = {'<registry version="2.1.0">\n<savegame><mod><local-voice-probe><vp>\n'}
	for _, k in ipairs(order) do out[#out + 1] = string.format('<%s value="%s"/>\n', k:match("([^.]+)$"), reg[k]) end
	out[#out + 1] = "</vp></local-voice-probe></mod></savegame>\n</registry>\n"
	local fh = io.open(dir .. "/savegame.xml", "wb")
	if fh then fh:write(table.concat(out)); fh:close() end
end
client = {}
assert(loadstring(assert(io.open(mod .. "/plan.lua")):read("*a")))()
local src = assert(io.open(mod .. "/main.lua")):read("*a")
src = src:gsub("^#version 2", "--"):gsub('#include "plan.lua"', "--")
assert(loadstring(src, "main.lua"))()
client.init()
local doneAt
while true do
	client.tick(1 / 60)
	flush()
	if client.st == "done" then
		doneAt = doneAt or clock()
		if clock() - doneAt > 3 then break end
	end
	ffi.C.Sleep(15)
end
print(string.format("sim: loaded %d, played %d, lined up %d, unloaded %d", loads, plays, skips, unloads))
