-- Offline stand-in for the game (LuaJIT), to test probe.py without it: runs the installed mod's main.lua
-- at ~60 fps. HasFile asks the real disk; Spawn reads the prefab file and makes one "body" with its desc
-- and tags (a file under the same name is REMEMBERED, to exercise that line of the report);
-- UiGetImageSize reads the PNG header. Writes <dir>/savegame.xml each frame.
--   luajit probes/textprobe/sim.lua <dir> <installed mod folder> [seconds]
local ffi = require("ffi")
ffi.cdef[[ void Sleep(uint32_t ms); ]]
local dir, mod, secs = arg[1], arg[2], tonumber(arg[3] or "25")
local reg, order = {}, {}
local function set(k, v) if reg[k] == nil then order[#order + 1] = k end reg[k] = tostring(v) end
SetString, SetInt = set, set
function ClearKey() reg, order = {}, {} end
local function path(p) return mod .. p:sub(4) end
local function slurp(p)
	local fh = io.open(path(p), "rb")
	if not fh then return nil end
	local s = fh:read("*a")
	fh:close()
	return s
end
function HasFile(p) return slurp(p) ~= nil end
local ents, cache = {}, {}
local function unxml(s) return (s:gsub("&quot;", '"'):gsub("&lt;", "<"):gsub("&gt;", ">"):gsub("&amp;", "&")) end
function Spawn(p)
	local s = cache[p] or assert(slurp(p), "no such prefab")
	cache[p] = s
	local desc, tags = s:match('<body desc="(.-)" tags="(.-)"/>')
	ents[#ents + 1] = {desc = unxml(desc), t = tags:match("t=(%x+)")}
	return {#ents}
end
function GetEntityType() return "body" end
function GetDescription(h) return ents[h].desc end
function GetTagValue(h, k) return k == "t" and ents[h].t or "" end
function Delete() end
function Vec(x, y, z) return {x, y, z} end
function Transform(p) return {pos = p} end
function UiGetImageSize(p)
	local s = slurp(p)
	local function be(i) local a, b, c, d = s:byte(i, i + 3); return ((a * 256 + b) * 256 + c) * 256 + d end
	return be(17), be(21)
end
for _, f in ipairs({"UiPush", "UiPop", "UiTranslate", "UiAlign", "UiFont", "UiColor", "UiRect", "UiText"}) do _G[f] = function() end end
function UiCenter() return 960 end
function UiGetTextSize(s) return #s * 10, 24 end
local function flush()
	local out = {'<registry version="2.1.0">\n<savegame><mod><local-text-probe><txp>\n'}
	for _, k in ipairs(order) do out[#out + 1] = string.format('<%s value="%s"/>\n', k:match("([^.]+)$"), reg[k]) end
	out[#out + 1] = "</txp></local-text-probe></mod></savegame>\n</registry>\n"
	local fh = io.open(dir .. "/savegame.xml", "wb")
	if fh then fh:write(table.concat(out)); fh:close() end
end
client = {}
local src = assert(io.open(mod .. "/main.lua")):read("*a"):gsub("^#version 2", "--")
assert(loadstring(src, "main.lua"))()
client.init()
local t0 = os.clock()
for _ = 1, secs * 60 do
	client.tick(1 / 60)
	client.draw()
	flush()
	ffi.C.Sleep(15)
end
print("sim done")
