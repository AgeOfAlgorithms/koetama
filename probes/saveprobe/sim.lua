-- Offline stand-in for the game (LuaJIT): runs mod/main.lua at ~60 fps against a mock registry, writes
-- <dir>/savegame.xml every FLUSH ms when a key changed and <dir>/log.txt for each Spawn, with wall-clock
-- times like the game's. For testing watch.py without the game:
--   luajit probes/saveprobe/sim.lua <dir> [flush_ms]      (phases shortened to 1 / 4 / 4 / 2 s)
local ffi = require("ffi")
ffi.cdef[[
typedef struct { uint16_t y, mo, dow, d, h, mi, s, ms; } SYSTEMTIME;
void GetLocalTime(SYSTEMTIME *t);
void Sleep(uint32_t ms);
int QueryPerformanceCounter(int64_t *c);
int QueryPerformanceFrequency(int64_t *f);
]]
local dir = arg[1] or "."
local FLUSH = tonumber(arg[2] or "0") / 1000
local st, c, f = ffi.new("SYSTEMTIME"), ffi.new("int64_t[1]"), ffi.new("int64_t[1]")
ffi.C.QueryPerformanceFrequency(f)
local function clock() ffi.C.QueryPerformanceCounter(c) return tonumber(c[0]) / tonumber(f[0]) end
local function stamp()
	ffi.C.GetLocalTime(st)
	return string.format("%02d:%02d:%02d.%03d000", st.h, st.mi, st.s, st.ms)
end

local reg, order, dirty, frame = {}, {}, false, 0
local function set(k, v)
	if reg[k] == nil then order[#order + 1] = k end
	if reg[k] ~= v then reg[k] = v; if k:find("^savegame") then dirty = true end end
end
function SetString(k, v) set(k, tostring(v)) end
function SetInt(k, v) set(k, tostring(math.floor(v))) end
function SetFloat(k, v) set(k, tostring(v)) end
function GetString(k) return reg[k] or "" end
function GetFloat(k) return tonumber(reg[k]) or 0 end
function ClearKey(k)
	for key in pairs(reg) do if key:sub(1, #k) == k then reg[key] = nil; dirty = true end end
	local o = {} for _, key in ipairs(order) do if reg[key] ~= nil then o[#o + 1] = key end end
	order = o
end
function Vec(x, y, z) return {x, y, z} end
function Transform(p) return {pos = p} end
function Delete() end
local log = assert(io.open(dir .. "/log.txt", "w"))
function Spawn(xml)
	log:write(string.format("%04d %s INFO bb12 [NoTag|LocalMod] Spawn: %s\n", frame, stamp(), xml))
	log:flush()
	return {1}
end
local t0 = clock()
function GetTime() return clock() - t0 end

local function flush()
	local out = {'<registry version="2.1.0">\n\t<savegame>\n\t\t<mod>\n\t\t\t<local-save-probe>\n'}
	local any = false
	for _, k in ipairs(order) do
		local name = k:match("^savegame%.mod%.svp%.(.+)$")
		if name then
			if not any then out[#out + 1] = "\t\t\t\t<svp>\n"; any = true end
			out[#out + 1] = string.format('\t\t\t\t\t<%s value="%s"/>\n', name, reg[k])
		end
	end
	if any then out[#out + 1] = "\t\t\t\t</svp>\n" end
	out[#out + 1] = "\t\t\t</local-save-probe>\n\t\t</mod>\n\t</savegame>\n</registry>\n"
	local fh = io.open(dir .. "/savegame.xml", "wb")
	if fh then fh:write(table.concat(out)); fh:close() end
end

server, client, shared = {}, {}, {}
local src = assert(io.open("probes/saveprobe/mod/main.lua")):read("*a")
src = src:gsub("^#version 2", "--"):gsub("local WAIT, A, B, C = 3, 20, 20, 10", "local WAIT, A, B, C = 1, 4, 4, 2")
assert(loadstring(src, "main.lua"))()
flush()
server.init()
local lastFlush = 0
while true do
	frame = frame + 1
	server.tick(1 / 60)
	if dirty and clock() - lastFlush >= FLUSH then flush(); dirty = false; lastFlush = clock() end
	if reg["svp.phase"] == "done" and GetTime() > 1 + 4 + 4 + 2 + 4 then break end
	ffi.C.Sleep(16)
end
log:close()
print("sim done, frames " .. frame)
